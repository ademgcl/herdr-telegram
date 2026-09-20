//! Watchdog reconcile tick: agent/shell transitions, stall scan,
//! dead-pane hygiene, and 1:1 tab↔topic titles. All herdr reads are
//! fail-closed — a degraded fetch skips its block, never reformats or
//! closes from defaults (mass-revert / mass-close on outage).
use crate::{
    handlers::reset::is_resetting,
    handlers::shell_common::{ShellReuse, classify_shell_reuse},
    herdr::client::{get_agent, list_agents},
    notifier::hygiene::{panes_once, reap_orphans},
    notifier::status::observe_status,
    state::AppState,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// Process-wide reconcile single-flight (reset-guard parity): the 60s
/// watchdog tick and the event-task resubscribe scan ("reconnect") run
/// on different tasks — `MissedTickBehavior::Skip` serializes ticks
/// against themselves only, never against the event task. The loser
/// skips (every scan is per-pane idempotent; the next tick covers), so
/// concurrent full scans never double RPC load or interleave
/// observe/limits/hygiene writes. Atomic flag, never a mutex held
/// across RPCs.
static RECONCILE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct ReconcileGuard;
impl Drop for ReconcileGuard {
    fn drop(&mut self) {
        RECONCILE_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

fn try_begin_reconcile() -> Option<ReconcileGuard> {
    RECONCILE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| ReconcileGuard)
}

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Some(_guard) = try_begin_reconcile() else {
        println!("[reconcile] skipped overlapping {src} scan (one already running)");
        return;
    };
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
    // scan runs in the tail AFTER the shell flips below — a pane that
    // quit since the last tick must scan as `shell` (skipped), never
    // with its stale agent status against fresh shell output (one-tick
    // STRONG-auth false-❗). Skipped on the silent seed (boot text can
    // mimic error banners); the next watchdog tick surfaces real stalls.

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
                    if crate::herdr::rpc::is_not_found(&e.to_string()) {
                        confirmed.push(pane);
                    } else {
                        // Blip keeps live (fail-closed): log so unmatched
                        // herdr vocab stays visible instead of blind.
                        eprintln!(
                            "[reconcile] kept {pane} on blip: {}",
                            crate::types::mask_home(&e.to_string())
                        );
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
                            // Live-only: a stopped corpse between
                            // mark_stopped() and map removal must not
                            // read as an active watcher (else Ignore
                            // misroutes to CancelJob/RetireVanished).
                            let has_job = s
                                .jobs
                                .lock()
                                .await
                                .get(&pane)
                                .is_some_and(|j| !j.is_stopped());
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
                                    // Split to `reconcile_vanished`
                                    // (300-line file limit).
                                    super::reconcile_vanished::retire_vanished(s, &pane, owed)
                                        .await;
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

    // DM flip + stall scan + hygiene + titles tail (split: 300-line file limit).
    super::reconcile_tail::reconcile_tail(s, &rows, &live_panes, &mut pane_list, silent).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconcile_guard_single_flight() {
        // First claim wins, overlap loses, drop releases.
        let g = try_begin_reconcile().expect("first scan claims");
        assert!(try_begin_reconcile().is_none());
        drop(g);
        assert!(try_begin_reconcile().is_some());
        RECONCILE_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}
