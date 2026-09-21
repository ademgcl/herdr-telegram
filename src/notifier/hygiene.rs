//! Dead-pane hygiene: mode-independent reaping of jobs, durable intent,
//! and per-pane maps for externally-closed panes. Split from `reconcile`
//! (300-line file limit).
use crate::{
    jobs::{persist, recover::recoverable},
    state::{
        AppState,
        guard::{
            BLOCKOP_STALE_SECS, MODELOP_STALE_SECS, SPAWNDEDUP_SECS, SPAWNOP_STALE_SECS,
            claim_stale, reap_stale,
        },
    },
};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "hygiene_panes.rs"]
mod hygiene_panes;
pub(crate) use hygiene_panes::{panes_once, prune_age};

/// Confirm suspected deaths against a fresh read (pure, tested): panes
/// missing from both reads are truly dead; panes the first read
/// transiently missed but the confirm sees must survive AND join the
/// retain set (pruning on one read's miss wipes a live session's
/// baselines/dedup and reaps its tap guard). Single source for the
/// double-confirm arm below.
pub(crate) fn confirm_deaths(
    first: &HashSet<String>,
    fresh: &HashSet<String>,
    dying: Vec<String>,
) -> (Vec<String>, HashSet<String>) {
    let out: Vec<String> = dying.into_iter().filter(|p| !fresh.contains(p)).collect();
    let mut retain: HashSet<String> = first.clone();
    for p in fresh {
        retain.insert(p.clone());
    }
    (out, retain)
}

/// Reap mode-independent orphans: DM-mode prompts can orphan jobs,
/// durable intent, and per-pane maps for externally-closed panes (no
/// topic mapping exists to trigger the forum close flow). Live panes
/// are skipped; truly gone ones are cancelled + cleared. Fail-open:
/// never wipe intents on a failed list call (Err) or a transient empty
/// Ok([]).
pub(crate) async fn reap_orphans(s: &AppState, pane_list: &mut Option<HashSet<String>>) {
    let mut known: Vec<String> = s.jobs.lock().await.keys().cloned().collect();
    known.extend(s.pending.lock().await.keys().cloned());
    // Focus-only corpses pin nothing above: a dead focus with no job,
    // intent, waiter or guard never entered `known`, so clear_pane never
    // ran and every bare DM prompt bricked on UNKNOWN_TARGET. Pin it.
    if let Some(f) = s.get_focus().await
        && !known.contains(&f)
    {
        known.push(f);
    }
    // Armed input waiters also pin a pane: a keywait/typewait for an
    // externally-closed shell (no job, no intent, DM mode) must die
    // with it instead of eating the next message as dead input.
    // runwait is deliberately EXCLUDED: it holds workspace ids, not
    // pane ids — admitting one would misread it as a dead pane and
    // wipe the armed shell-run waiter it was meant to protect.
    known.extend(s.keywait.lock().await.values().map(|(p, _)| p.clone()));
    known.extend(s.typewait.lock().await.values().map(|(p, _)| p.clone()));
    // runwait expires by AGE, never by pane-liveness (see above): an
    // armed waiter firing arbitrarily later would execute stale input
    // as a shell command. Fail-closed expiry, same corpse bound style
    // as the op guards. Key/type waiters expire by age too (a stale K
    // arm keys into live work, a stale Type arm answers a dead question)
    // — pane-death reap above still applies first. Split to `prune_age`
    // (300-line file limit): waiters, notice stamps, debounce arms, and
    // done stamps all prune there.
    prune_age(s).await;
    // Guard-only wedges pin too: a tap that consumed its waiter (no
    // job, no intent) must still reach the reap below, never idle-skip.
    // NOTE: `spawnop` is deliberately EXCLUDED — its keys are synthetic
    // (`spawn:<chat>:<msg>`), never live panes, so pane-liveness reaping
    // would wipe in-flight spawns and double-mint billable resources.
    // Spawn guards expire by AGE only — pruned up front so the
    // `known.is_empty()` early return below never strands them while idle.
    {
        let now = std::time::Instant::now();
        {
            let mut m = s.spawnop.lock().await;
            if !m.is_empty() {
                let before = m.len();
                m.retain(|_, at| !claim_stale(*at, now, SPAWNOP_STALE_SECS));
                if m.len() != before {
                    eprintln!("[hygiene] reaped stale spawn guard");
                }
            }
        }
        {
            let mut m = s.spawndone.lock().await;
            if !m.is_empty() {
                m.retain(|_, at| !claim_stale(*at, now, SPAWNDEDUP_SECS));
            }
        }
    }
    known.extend(s.blockop.lock().await.keys().cloned());
    known.extend(s.modelop.lock().await.keys().cloned());
    // Blocked-only corpses pin too: a DM pane with only a blocked
    // card (no job, no intent) must still reach the reap below, or its
    // dead ⛔ buttons stay tappable forever (the live-only retains
    // further down would prune tracking without stripping).
    known.extend(s.blocked_sig.lock().await.keys().cloned());
    let blocked_keys: Vec<String> = s.blocked_card.lock().await.keys().cloned().collect();
    for p in blocked_keys {
        if !known.contains(&p) {
            known.push(p);
        }
    }
    // No early return on empty `known`: an idle bot (no jobs, intents,
    // waiters, or guards) must still prune live-only maps below, or dead
    // panes leak their seen/history/status/typing entries forever. The
    // per-pane retire loop simply no-ops while the retains still run.
    let reaping = !known.is_empty();
    // Injected lists (tests) skip the double-confirm RPC: the caller
    // already supplied the authoritative live set, and a forced fresh
    // read would hit a nonexistent socket (fail-open keeps all, so no
    // injected death could ever reap). Prod enters with None (the tick
    // resets the shared cache before this call) so suspected deaths
    // always re-confirm fresh.
    let injected = pane_list.is_some();
    match panes_once(s, pane_list).await {
        Some(live) if live.is_empty() => {
            eprintln!("[reconcile] pane list empty, keeping intents");
        }
        Some(mut live) => {
            // Retain live-only (not live∪known): known includes
            // the just-cleared dead panes, so ∪ would keep
            // everything clear_pane missed.
            // Double-confirm before ANY live-only retain, even when idle
            // or dying is empty: the retains below prune idle-only maps
            // (seen/history/status) for panes that never enter `known`,
            // so a single transient `list_panes` miss wipes a live
            // session's baseline and reposts scrollback as fresh. Both
            // reads must miss to retire; the union joins the retain set.
            if !injected {
                let first = live.clone();
                *pane_list = None;
                match panes_once(s, pane_list).await {
                    Some(fresh) if !fresh.is_empty() => {
                        let (_, union) = confirm_deaths(&live, &fresh, Vec::new());
                        for p in &union {
                            live.insert(p.clone());
                        }
                    }
                    Some(_) => {
                        eprintln!("[reconcile] confirm empty, keeping all");
                        *pane_list = Some(first);
                        // Fail-open includes the retains below: `live`
                        // is the suspect first read, so pruning maps
                        // against it would wipe live panes' baselines
                        // and repost scrollback as fresh. Skip the tick.
                        return;
                    }
                    _ => {
                        eprintln!("[reconcile] confirm read failed, keeping all");
                        *pane_list = Some(first.clone());
                        // Same fail-open as above: never prune on a
                        // single partial read.
                        return;
                    }
                }
            }
            if reaping {
                let dying: Vec<String> = known
                    .iter()
                    .filter(|p| !live.contains(*p))
                    .cloned()
                    .collect();
                for pane in &dying {
                    // Job-only retire: the durable intent is boot-recover's
                    // reporter for dead panes — a loud retire would wipe it
                    // the same tick the forum branch restored it. Waiters
                    // die via clear_pane, maps via the retains; lingering
                    // intent is bounded by the 24h stale drop. Strip dead
                    // ⛔ buttons first (contention-silent, shared helper).
                    crate::handlers::dialog::resolve_cards_unless_held(s, pane).await;
                    s.cancel_job_only_for(pane).await;
                    s.clear_pane(pane).await;
                }
            }
            s.seen.lock().await.retain(|p, _| live.contains(p));
            s.last_done.lock().await.retain(|p, _| live.contains(p));
            // History is RAM-only + bounded per pane, but history-only
            // panes (no job/intent) never enter `known` above — retain
            // live-only or dead panes leak 20 rows each forever.
            s.history.lock().await.retain(|p, _| live.contains(p));
            // Stale dead-pane intent: boot-recover drops >24h intents,
            // but without a restart in-uptime dead intents accumulate
            // forever. Same 24h bound here, persisted like any clear.
            {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let snap = {
                    let mut m = s.pending.lock().await;
                    let before = m.len();
                    m.retain(|_, pp| recoverable(pp.started_unix, now));
                    if m.len() == before {
                        None
                    } else {
                        Some(m.clone())
                    }
                };
                if let Some(snap) = snap {
                    persist::save_file(&persist::store_path(), &snap);
                }
            }
            // status+last_change under one scope (order status→last_change,
            // no awaits inside): an event inserting between torn retains
            // would orphan status without its change instant and skip
            // flap-collapse on the next bounce.
            {
                let mut st = s.status.lock().await;
                let mut lc = s.last_change.lock().await;
                st.retain(|p, _| live.contains(p));
                lc.retain(|p, _| live.contains(p));
            }
            s.limit_alert.lock().await.retain(|p, _| live.contains(p));
            s.limit_seen.lock().await.retain(|p, _| live.contains(p));
            s.limit_miss.lock().await.retain(|p, _| live.contains(p));
            s.limit_send_cool
                .lock()
                .await
                .retain(|p, _| live.contains(p));
            s.debounce.lock().await.retain(|p, _| live.contains(p));
            s.blocked_sig.lock().await.retain(|p, _| live.contains(p));
            s.blocked_card.lock().await.retain(|p, _| live.contains(p));
            // Kind memory is mapping-less by design (shells never mint):
            // dead entries never pass remove_mapping_if_thread, so they
            // retain live-only here like every other per-pane map.
            s.topics.prune_kinds(&live);
            // Shell generations are pane-scoped like the maps above.
            s.shell_gen.lock().await.retain(|p, _| live.contains(p));
            // Corpse-tap reap: a wedged OpGuard (drop lost the lock race)
            // must never brick answers until restart — every answer path
            // refuses while blockop holds the pane. Legit taps hold
            // minutes (bounded RPC timeouts, worst ≈3min stacked); anything
            // older is dead. Model switches pile longer (own threshold),
            // same guarantee. Peeks self-evict too — this tick is only
            // the backstop. Log after drop (never hold a guard across I/O).
            // (Spawn guards pruned up front — single site, before the
            // `known.is_empty()` early return — never pane-liveness.)
            {
                let now = std::time::Instant::now();
                let reaped_tap = {
                    let mut m = s.blockop.lock().await;
                    reap_stale(&mut m, &live, now, BLOCKOP_STALE_SECS)
                };
                for p in reaped_tap {
                    eprintln!("[hygiene] reaped stale tap guard for {p}");
                }
                let reaped_model = {
                    let mut m = s.modelop.lock().await;
                    reap_stale(&mut m, &live, now, MODELOP_STALE_SECS)
                };
                for p in reaped_model {
                    eprintln!("[hygiene] reaped stale model guard for {p}");
                }
            }
            // Typing tasks for dead panes: ownership-checked stop, never raw
            // abort — a remint racing the tick keeps its task (a raw
            // extract_if+abort could kill a successor's task while its job
            // lives). Dead panes have no job by now (cancelled above), so
            // this still stops them; live/flapped panes are untouched.
            let drop_typing: Vec<String> = s
                .typing_tasks
                .lock()
                .await
                .keys()
                .filter(|p| !live.contains(*p))
                .cloned()
                .collect();
            for pane in &drop_typing {
                s.stop_typing_unless_owned(pane).await;
            }
            // Reply targets are NEVER pruned (not even for dead panes):
            // a DM reply to a corpse card must fail visibly via the
            // shell fallback, never silently reroute into the focused
            // live agent. Dead entries age out via the 512-cap overflow.
        }
        None => {}
    }
}

#[cfg(test)]
#[path = "hygiene_tests.rs"]
mod tests;
