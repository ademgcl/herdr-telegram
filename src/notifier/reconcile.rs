//! Watchdog reconcile tick: agent/shell transitions, stall scan,
//! dead-pane hygiene, and 1:1 tab↔topic titles. All herdr reads are
//! fail-closed — a degraded fetch skips its block, never reformats or
//! closes from defaults (mass-revert / mass-close on outage).
use crate::{
    handlers::reset::is_resetting,
    handlers::shell_common::{ShellReuse, classify_shell_reuse},
    herdr::client::{get_agent, list_agents, read_shell_output},
    jobs::finalize::report,
    notifier::hygiene::{panes_once, reap_orphans},
    notifier::limits::scan_limits,
    notifier::status::observe_status,
    state::AppState,
};
use std::collections::HashSet;

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    // While reset runs, topic lifecycle belongs to reset: a watchdog tick
    // opening topics and posting cards would race delete/create and cause
    // a 429 storm. Hygiene still runs (no topic writes) so status, limits
    // and dead-pane reaps never black out behind a paced reset; the first
    // tick after reset catches everything else.
    if is_resetting() {
        println!("[reconcile] degraded ({src}): paced reset in progress");
        let mut pane_list: Option<HashSet<String>> = None;
        reap_orphans(s, &mut pane_list).await;
        return;
    }
    let Ok(rows) = list_agents(&s.cfg.socket).await else {
        // Degraded herdr must not stall hygiene: age-prunes (waiters,
        // guards, notice stamps, 24h intent) run without any RPC, and
        // the live-wipe half safely no-ops on a failed pane list.
        // (Reset still skips above — it owns topic lifecycle.)
        let mut pane_list: Option<HashSet<String>> = None;
        reap_orphans(s, &mut pane_list).await;
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
    // scan non-shell panes for the banner directly (immediate quota/auth
    // on any status — herdr can sample idle mid-retry; gated still needs
    // working), backstopping prompt-owned panes with parked watchers.
    // Skipped on the silent seed (boot text can mimic error banners); the
    // next watchdog tick — 60s later — surfaces real stalls anyway.
    if !silent {
        scan_limits(s).await;
    }

    // Stored panes with no agent are shells (quit) or dead (closed).
    // Shells keep their topic with the shell badge and zero alerts;
    // only truly gone panes get closed. Fail-open: a herdr hiccup must
    // never read as "everything is dead" (wiped topics + intents).
    // Single `list_panes` per tick (shared with the hygiene block
    // below): 1 RPC, not 2. Forum-only: in DM mode a quit-to-shell pane
    // keeps its last agent status (fully-dead panes are still reaped by
    // hygiene below); shell reuse under the same name inherits episode
    // dedup for up to one remind window — accepted, DM has no topics.
    let mut pane_list: Option<HashSet<String>> = None;
    if s.cfg.forum.is_some() {
        let stored = s.topics.all_mappings();
        let missing: Vec<String> = stored
            .keys()
            .filter(|p| !live_panes.contains(*p))
            .cloned()
            .collect();
        // Vanish confirm: a single `list_agents` dropout (Ok but
        // partial) must not flip a live agent to shell (false "quit"
        // card + watcher retire). `get_agent` re-proves each suspect —
        // only the still-missing proceed. Fail-closed: a blip (timeout,
        // unreachable) keeps the pane live; only a not-found answer
        // confirms death. Suspects-only, so the steady state costs zero
        // extra RPCs.
        let mut confirmed = Vec::with_capacity(missing.len());
        for pane in missing {
            match get_agent(&s.cfg.socket, &pane).await {
                Ok(_) => {
                    live_panes.insert(pane);
                }
                Err(e) => {
                    let msg = e.to_string().to_lowercase();
                    if msg.contains("not found")
                        || msg.contains("no such")
                        || msg.contains("unknown pane")
                    {
                        confirmed.push(pane);
                    } else {
                        // Blip keeps live (fail-closed): log so unmatched
                        // herdr vocab stays visible instead of blind.
                        eprintln!("[reconcile] kept {pane} on blip: {}", crate::types::mask_home(&e.to_string()));
                        live_panes.insert(pane);
                    }
                }
            }
        }
        let missing = confirmed;
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
                            // dedup (working flicker no longer clears — only
                            // shell/death/confirmed-clean reads do).
                            s.clear_limit_episode(&pane).await;
                            // Pruned (human-deleted) topics retire the dialog
                            // before the shelled pane reposts anything.
                            if s.topics.mark_shell(&pane).await {
                                crate::handlers::dialog::retire_dialog(s, &pane).await;
                            }
                            // Flip owns no card work below (Ignore): retire
                            // live blocked buttons/sig here (status.rs
                            // parity) — else the old agent's buttons bait
                            // taps in the shelled topic and a same-content
                            // re-block goes silent on the stale sig.
                            if s.block_held(&pane).await {
                                s.blocked_sig.lock().await.remove(&pane);
                            } else {
                                crate::handlers::dialog::resolve_cards(s, &pane).await;
                            }
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
                                    // Claim the report slot first: a concurrent
                                    // tick (watchdog + event reconnect) must
                                    // not double-report the same quit — the
                                    // loser sees shell and stands down.
                                    let prev_status = {
                                        s.status
                                            .lock()
                                            .await
                                            .insert(pane.clone(), "shell".to_string())
                                    };
                                    if prev_status.as_deref() == Some("shell") {
                                        continue;
                                    }
                                    // Turned-over intent (submit raced the
                                    // flip): retire the dead watcher only,
                                    // preserving the new pending — quiet
                                    // would wipe it with no restore.
                                    if !mine {
                                        s.cancel_job_only_for(&pane).await;
                                        continue;
                                    }
                                    s.cancel_jobs_for_quiet(&pane).await;
                                    if let Some(pp) = owed {
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
                                            // CAS: an observation landing during
                                            // the RPCs wins — never clobber it.
                                            let mut st = s.status.lock().await;
                                            if st.get(&pane).map(|v| v == "shell").unwrap_or(false) {
                                                st.insert(pane.clone(), "shell".to_string());
                                            }
                                        } else {
                                            // Intent was already cancelled above:
                                            // keep it so boot-recover retries
                                            // the notice instead of losing it.
                                            // CAS restore so the next tick
                                            // retries instead of going quiet.
                                            {
                                                let mut st = s.status.lock().await;
                                                if st.get(&pane).map(|v| v == "shell").unwrap_or(false) {
                                                    match prev_status {
                                                        Some(p) => {
                                                            st.insert(pane.clone(), p);
                                                        }
                                                        None => {
                                                            st.remove(&pane);
                                                        }
                                                    }
                                                }
                                            }
                                            // Guarded like the dead-close restore:
                                            // a submit racing the RPCs wins
                                            // (atomic check-and-set: a
                                            // check-then-remember across
                                            // awaits would overwrite it).
                                            s.remember_pending_cas(
                                                &pane,
                                                (pp.chat, pp.thread, &pp.prompt),
                                                (pp.chat, pp.thread, &pp.prompt),
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
                        // Split to `reconcile_close` (300-line file limit).
                        } else if super::reconcile_close::close_dead_pane(s, &pane).await {
                            continue;
                        }
                    }
                }
                None => {}
            }
        }
    }

    // DM flip + hygiene + titles tail (split: 300-line file limit).
    super::reconcile_tail::reconcile_tail(s, &rows, &live_panes, &mut pane_list).await;
}
