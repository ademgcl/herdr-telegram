//! Dead-pane hygiene: mode-independent reaping of jobs, durable intent,
//! and per-pane maps for externally-closed panes. Split from `reconcile`
//! (300-line file limit).
use crate::{
    herdr::client::list_panes,
    jobs::{persist, recover::recoverable},
    state::{
        AppState,
        guard::{
            BLOCKOP_STALE_SECS, KEYWAIT_STALE_SECS, MODELOP_STALE_SECS, RUNWAIT_STALE_SECS,
            TYPEWAIT_STALE_SECS, claim_stale, reap_stale,
        },
    },
};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Cached single `list_panes` per tick (shared with the forum block):
/// 1 RPC, not 2. Fail-open: Err keeps everything.
pub(crate) async fn panes_once(
    s: &AppState,
    cache: &mut Option<HashSet<String>>,
) -> Option<HashSet<String>> {
    if let Some(p) = cache {
        return Some(p.clone());
    }
    match list_panes(&s.cfg.socket).await {
        Ok(l) => {
            let set: HashSet<String> = l.into_iter().collect();
            *cache = Some(set.clone());
            Some(set)
        }
        Err(e) => {
            eprintln!("[reconcile] pane list failed, keeping topics: {e}");
            None
        }
    }
}

/// DM-mode shell flip: no topics exist, but `status` still drives the
/// limit scanner — a PC-side quit would keep its last agent status
/// forever and quota words in ordinary shell output would buzz false
/// ❗ cards. Flip shell-reused panes (status + episode only — no topic,
/// no report); dead panes stay for `reap_orphans` below. Split from
/// `reconcile` (300-line file limit).
pub(crate) async fn flip_dm_shells(
    s: &AppState,
    live_panes: &HashSet<String>,
    pane_list: &mut Option<HashSet<String>>,
) {
    let missing: Vec<String> = {
        let st = s.status.lock().await;
        st.keys()
            .filter(|p| !live_panes.contains(*p) && st.get(*p).map(|v| v != "shell").unwrap_or(false))
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let Some(panes) = panes_once(s, pane_list).await else {
        return;
    };
    if panes.is_empty() {
        return;
    }
    for pane in missing {
        if panes.contains(&pane) {
            s.status.lock().await.insert(pane.clone(), "shell".to_string());
            s.clear_limit_episode(&pane).await;
        }
    }
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
    // — pane-death reap above still applies first.
    {
        let now = std::time::Instant::now();
        s.runwait
            .lock()
            .await
            .retain(|_, (_, at)| !claim_stale(*at, now, RUNWAIT_STALE_SECS));
        s.keywait
            .lock()
            .await
            .retain(|_, (_, at)| !claim_stale(*at, now, KEYWAIT_STALE_SECS));
        s.typewait
            .lock()
            .await
            .retain(|_, (_, at)| !claim_stale(*at, now, TYPEWAIT_STALE_SECS));
    }
    // Guard-only wedges pin too: a tap that consumed its waiter (no
    // job, no intent) must still reach the reap below, never idle-skip.
    known.extend(s.blockop.lock().await.keys().cloned());
    known.extend(s.modelop.lock().await.keys().cloned());
    if known.is_empty() {
        return;
    }
    match panes_once(s, pane_list).await {
        Some(live) if live.is_empty() => {
            eprintln!("[reconcile] pane list empty, keeping intents");
        }
        Some(live) => {
            // Retain live-only (not live∪known): known includes
            // the just-cleared dead panes, so ∪ would keep
            // everything clear_pane missed.
            for pane in &known {
                if !live.contains(pane) {
                    // Job-only retire: the durable intent is
                    // boot-recover's reporter for dead panes
                    // ("pane gone before reply arrived") — a loud
                    // retire would wipe it the same tick the
                    // forum branch above restored it, and a
                    // "✋ cancelled" card on a dead pane is noise.
                    // Waiters still die via clear_pane below, maps
                    // via the retains; lingering intent is bounded
                    // by recover's 24h stale drop.
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
            // Corpse-tap reap: a wedged OpGuard (drop lost the lock race)
            // must never brick answers until restart — every answer path
            // refuses while blockop holds the pane. Legit taps hold
            // minutes (bounded RPC timeouts, worst ≈3min stacked); anything
            // older is dead. Model switches pile longer (own threshold),
            // same guarantee. Peeks self-evict too — this tick is only
            // the backstop. Log after drop (never hold a guard across I/O).
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
