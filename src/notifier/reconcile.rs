use crate::{
    handlers::shell_common::{ShellReuse, classify_shell_reuse},
    handlers::titles::sync_titles_with,
    herdr::client::{list_agents, list_workspaces, read_shell_output},
    herdr::labels::pane_facts,
    jobs::finalize::report,
    notifier::hygiene::{panes_once, reap_orphans},
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
    // Single `list_panes` per tick (shared with the hygiene block
    // below): 1 RPC, not 2.
    let mut pane_list: Option<HashSet<String>> = None;
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
                            // Already-shell panes run shell commands, not
                            // agents: their pending intent is live work.
                            // Only a fresh agent→shell flip owns the
                            // vanished-agent retire below. Status is
                            // volatile (empty at boot), so an unknown
                            // status consults the durable shell marker
                            // (creation tag/icon) before crying flip.
                            let was_shell = match s.status.lock().await.get(&pane).cloned() {
                                Some(st) => st == "shell",
                                None => s.topics.is_shell_tagged(&pane),
                            };
                            // Agent gone (shell reuse): its stall episode dies here
                            // or the next agent on this pane name inherits stale
                            // dedup (see status.rs working→* clear).
                            s.clear_limit_episode(&pane).await;
                            s.topics.mark_shell(&pane).await;
                            // A prompt owed to the vanished agent (quit on
                            // the PC, no Telegram /quit) must not spin its
                            // watcher in get_agent backoff forever: retire
                            // it, surfacing the shell tail as the reply.
                            // On already-shell panes the intent is an
                            // active shell command — a blind retire here
                            // ate it every tick, so long runs posted only
                            // their start card and never the follow-up.
                            let owed = s.pending.lock().await.get(&pane).cloned();
                            let has_job = s.jobs.lock().await.contains_key(&pane);
                            match classify_shell_reuse(was_shell, owed.is_some(), has_job) {
                                ShellReuse::Ignore => {
                                    s.status
                                        .lock()
                                        .await
                                        .insert(pane.clone(), "shell".to_string());
                                }
                                ShellReuse::CancelJob => {
                                    s.status
                                        .lock()
                                        .await
                                        .insert(pane.clone(), "shell".to_string());
                                    s.cancel_job_only_for(&pane).await;
                                }
                                ShellReuse::RetireVanished => {
                                    // Race verdict BEFORE retiring: quiet clears
                                    // the slot, so a post-quiet check would
                                    // always read false and drop the notice.
                                    // A racer-retired intent has its own ack.
                                    // (Micro-race: submit between check and
                                    // retire — microseconds, no RPC between.)
                                    let mine = match &owed {
                                        Some(pp) => {
                                            s.pending_matches(&pane, pp.chat, pp.thread, &pp.prompt)
                                                .await
                                        }
                                        // Job-only flip: no competing intent.
                                        None => true,
                                    };
                                    // Quiet retire: the pane died (agent quit
                                    // to shell), so the parked watcher must
                                    // exit silently — a loud cancel would
                                    // post "✋ cancelled" next to the quit
                                    // notice below (two cards, one prompt).
                                    s.cancel_jobs_for_quiet(&pane).await;
                                    if let Some(pp) = owed {
                                        if !mine {
                                            continue;
                                        }
                                        let tail = read_shell_output(&s.cfg.socket, &pane, 60)
                                            .await
                                            .map(|t| t.trim().to_string())
                                            .unwrap_or_default();
                                        // Always notify (even with an empty
                                        // tail): the owed prompt retires
                                        // here, silently dropping it would
                                        // miss the reply with no retry.
                                        let msg = if tail.is_empty() {
                                            "agent quit to shell.".to_string()
                                        } else {
                                            format!("agent quit to shell — last output:\n{tail}")
                                        };
                                        if report(s, pp.chat, pp.thread, &pane, &msg).await {
                                            s.status
                                                .lock()
                                                .await
                                                .insert(pane.clone(), "shell".to_string());
                                        } else {
                                            // Intent was already cancelled above:
                                            // keep it so boot-recover retries
                                            // the notice instead of losing it.
                                            // Status stays un-shell so the
                                            // next tick retries the retire
                                            // instead of going quiet.
                                            s.remember_pending(
                                                &pane, pp.chat, pp.thread, &pp.prompt,
                                            )
                                            .await;
                                        }
                                    } else {
                                        s.status
                                            .lock()
                                            .await
                                            .insert(pane.clone(), "shell".to_string());
                                    }
                                }
                            }
                        // Dead-pane close is silent (no card), so it never waits
                        // for a non-silent tick — orphans from a restart close on
                        // the seed pass instead of lingering a full cycle.
                        // Compare-and-delete: a remint between snapshot and
                        // close must survive the outer remove. Quiet retire:
                        // loud's "✋ cancelled" card would break the silence.
                        } else {
                            let thread = s.topics.all_mappings().get(&pane).copied();
                            // Snapshot the owed intent: the silent close below
                            // retires it, but boot-recover's gone-notice is
                            // the designed reporter for dead panes — restore
                            // it so the reply still arrives next boot
                            // (bounded by recover's 24h stale drop). The flap
                            // self-terminates: a successful close drops the
                            // mapping, so this runs at most once more.
                            let owed = s.pending.lock().await.get(&pane).cloned();
                            if s.topics.close_topic(&pane).await {
                                // Some(thread): compare-delete a remint-safe
                                // prune. None: mapping already gone (pruned
                                // inside or raced) — no-op, never wipe a
                                // concurrent remint blindly.
                                if let Some(t) = thread {
                                    s.topics.remove_mapping_if_thread(&pane, t);
                                }
                            }
                            s.cancel_jobs_for_quiet(&pane).await;
                            s.clear_pane(&pane).await;
                            if let Some(pp) = owed {
                                s.remember_pending(&pane, pp.chat, pp.thread, &pp.prompt)
                                    .await;
                            }
                        }
                    }
                }
                None => {}
            }
        }
    }

    // Mode-independent dead-pane hygiene (jobs, intent, per-pane maps
    // for externally-closed panes). Fail-open on Err/empty (see hygiene).
    reap_orphans(s, &mut pane_list).await;

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
