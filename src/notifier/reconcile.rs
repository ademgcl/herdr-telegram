use crate::{
    handlers::titles::sync_titles_with,
    herdr::client::{list_agents, list_panes, list_workspaces, read_shell_output},
    herdr::labels::pane_facts,
    jobs::finalize::report,
    notifier::limits::scan_limits,
    notifier::status::observe_status,
    state::AppState,
};
use std::collections::HashSet;

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Ok(rows) = list_agents(&s.cfg.socket).await else {
        return;
    };

    let mut live_panes = HashSet::new();

    for r in &rows {
        live_panes.insert(r.pane.clone());
        // Silent or not, this ensures the topic + pin exist; non-silent
        // also posts cards for genuine transitions.
        observe_status(s, &r.pane, &r.status, silent, src).await;
    }

    // Rate-limit stalls never transition (herdr reports `working` while
    // opencode retries internally), so the status path above stays mute:
    // scan working panes for the banner directly. Skipped on the silent
    // seed (boot text can mimic error banners); the next watchdog tick
    // — 60s later — surfaces real stalls anyway.
    if !silent {
        scan_limits(s).await;
    }

    // Stored panes with no agent are shells (quit) or dead (closed).
    // Shells keep their topic with the shell badge and zero alerts;
    // only truly gone panes get closed. Fail-open: a herdr hiccup must
    // never read as "everything is dead" (wiped topics + intents).
    // Single `list_panes` per tick (shared with the DM hygiene block
    // below): 1 RPC, not 2.
    let mut pane_list: Option<HashSet<String>> = None;
    async fn panes_once(
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
    if s.cfg.forum.is_some() {
        let stored = s.topics.all_mappings();
        let missing: Vec<String> = stored
            .keys()
            .filter(|p| !live_panes.contains(*p))
            .cloned()
            .collect();
        if !missing.is_empty() {
            match panes_once(s, &mut pane_list).await {
                Some(panes) if panes.is_empty() => {
                    // Transient Ok([]) must not read as "everything dead"
                    // (wiped topics + intents); same fail-open as Err.
                    eprintln!("[reconcile] pane list empty, keeping topics");
                }
                Some(panes) => {
                    for pane in missing {
                        if panes.contains(&pane) {
                            s.status
                                .lock()
                                .await
                                .insert(pane.clone(), "shell".to_string());
                            // Agent gone (shell reuse): its stall episode dies here
                            // or the next agent on this pane name inherits stale
                            // dedup (see status.rs working→* clear).
                            s.clear_limit_episode(&pane).await;
                            s.topics.mark_shell(&pane).await;
                            // A prompt owed to the vanished agent (quit on
                            // the PC, no Telegram /quit) must not spin its
                            // watcher in get_agent backoff forever: retire
                            // it, surfacing the shell tail as the reply.
                            let owed = s.pending.lock().await.get(&pane).cloned();
                            if owed.is_some() || s.jobs.lock().await.contains_key(&pane) {
                                s.cancel_jobs_for(&pane).await;
                                if let Some(pp) = owed
                                    && let Ok(tail) =
                                        read_shell_output(&s.cfg.socket, &pane, 60).await
                                {
                                    let tail = tail.trim().to_string();
                                    if !tail.is_empty() {
                                        let msg =
                                            format!("agent quit to shell — last output:\n{tail}");
                                        if !report(s, pp.chat, pp.thread, &pane, &msg).await {
                                            // Intent was already cancelled above:
                                            // keep it so boot-recover retries
                                            // the notice instead of losing it.
                                            s.remember_pending(
                                                &pane, pp.chat, pp.thread, &pp.prompt,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                        // Dead-pane close is silent (no card), so it never waits
                        // for a non-silent tick — orphans from a restart close on
                        // the seed pass instead of lingering a full cycle.
                        // Compare-and-delete: a remint between snapshot and
                        // close must survive the outer remove.
                        } else {
                            let thread = s.topics.all_mappings().get(&pane).copied();
                            if s.topics.close_topic(&pane).await {
                                // Some(thread): compare-delete a remint-safe
                                // prune. None: mapping already gone (pruned
                                // inside or raced) — no-op, never wipe a
                                // concurrent remint blindly.
                                if let Some(t) = thread {
                                    s.topics.remove_mapping_if_thread(&pane, t);
                                }
                            }
                            s.cancel_jobs_for(&pane).await;
                            s.clear_pane(&pane).await;
                        }
                    }
                }
                None => {}
            }
        }
    }

    // Mode-independent dead-pane hygiene: DM-mode prompts can orphan
    // jobs, durable intent, and per-pane maps for externally-closed
    // panes (no topic mapping exists to trigger the forum close flow).
    // Live panes are skipped; truly gone ones are cancelled + cleared.
    // Fail-open: never wipe intents on a failed list call (Err) or a
    // transient empty Ok([]).
    {
        let mut known: Vec<String> = s.jobs.lock().await.keys().cloned().collect();
        known.extend(s.pending.lock().await.keys().cloned());
        // Armed input waiters also pin a pane: a keywait/typewait for an
        // externally-closed shell (no job, no intent, DM mode) must die
        // with it instead of eating the next message as dead input.
        known.extend(s.keywait.lock().await.values().cloned());
        known.extend(s.runwait.lock().await.values().cloned());
        known.extend(s.typewait.lock().await.values().cloned());
        if !known.is_empty() {
            match panes_once(s, &mut pane_list).await {
                Some(live) if live.is_empty() => {
                    eprintln!("[reconcile] pane list empty, keeping intents");
                }
                Some(live) => {
                    // Retain live-only (not live∪known): known includes
                    // the just-cleared dead panes, so ∪ would keep
                    // everything clear_pane missed.
                    for pane in &known {
                        if !live.contains(pane) {
                            s.cancel_jobs_for(pane).await;
                            s.clear_pane(pane).await;
                        }
                    }
                    s.status.lock().await.retain(|p, _| live.contains(p));
                    s.seen.lock().await.retain(|p, _| live.contains(p));
                    s.last_done.lock().await.retain(|p, _| live.contains(p));
                    // Atomic with status (order status→last_change).
                    s.last_change.lock().await.retain(|p, _| live.contains(p));
                    s.limit_alert.lock().await.retain(|p, _| live.contains(p));
                    s.limit_seen.lock().await.retain(|p, _| live.contains(p));
                    s.limit_miss.lock().await.retain(|p, _| live.contains(p));
                    s.debounce.lock().await.retain(|p, _| live.contains(p));
                    s.blocked_sig.lock().await.retain(|p, _| live.contains(p));
                    s.modelop.lock().await.retain(|p| live.contains(p));
                    s.blockop.lock().await.retain(|p| live.contains(p));
                    // Typing tasks for dead panes: abort, don't leak.
                    for (_, h) in s
                        .typing_tasks
                        .lock()
                        .await
                        .extract_if(|p, _| !live.contains(p))
                        .collect::<Vec<_>>()
                    {
                        h.abort();
                    }
                    // Reply targets pointing at dead panes (order
                    // torder→targets, as in remember).
                    {
                        let mut ord = s.torder.lock().await;
                        let mut map = s.targets.lock().await;
                        map.retain(|_, p| live.contains(p));
                        let live_keys: std::collections::HashSet<(i64, i64)> =
                            map.keys().cloned().collect();
                        ord.retain(|k| live_keys.contains(k));
                    }
                }
                None => {}
            }
        }
    }

    // 1:1 pane↔topic titles (herdr labels win here; native TG renames
    // flow back via forum_topic_edited). Reuses this tick's rows plus
    // one spaces/facts fetch — no extra list_agents per tick.
    if s.cfg.forum.is_some() {
        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        if let Ok(facts) = pane_facts(&s.cfg.socket).await {
            sync_titles_with(s, &rows, &spaces, &facts).await;
        }
    }
}
