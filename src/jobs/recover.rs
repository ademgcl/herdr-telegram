use super::runner::watch_job;
use crate::{
    herdr::client::{get_agent, list_agents, read_screen_adaptive},
    jobs::finalize::report,
    jobs::job::Job,
    state::AppState,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Boot-rearm eligibility, pure so tests pin it: stale (>24h) or
/// future-dated (>1h ahead — a clock jump forward then corrected would
/// otherwise re-arm a dead prompt on every boot forever) intents never
/// re-arm.
pub fn recoverable(started_unix: u64, now: u64) -> bool {
    now.saturating_sub(started_unix) <= 86400 && started_unix <= now.saturating_add(3600)
}

/// Stale-drop clear verdict (pure, tested): delivery-gated, but bounded
/// so a dead Telegram never retries forever. Clear when the notice
/// landed, or when the intent is older than 7d / further than 7d in the
/// future (clock-jump corpse) — disk + boot scans stay bounded either
/// way. Single source for the stale arm below.
pub const STALE_KEEP_MAX_SECS: u64 = 7 * 86400;

pub fn stale_drop_should_clear(delivered: bool, started_unix: u64, now: u64) -> bool {
    if delivered {
        return true;
    }
    if now.saturating_sub(started_unix) > STALE_KEEP_MAX_SECS {
        return true;
    }
    if started_unix.saturating_sub(now) > STALE_KEEP_MAX_SECS {
        return true;
    }
    false
}

/// Atomic watcher claim: check + insert under the caller's single
/// jobs-lock hold. Returns false when a LIVE watcher already owns the pane
/// (a concurrent re-arm won the race) — caller must stand down, never
/// run two watchers. A stopped corpse never blocks a re-arm (enqueue
/// leaves stopped jobs in the map; they are replaced, never reused).
/// Pure over the map so tests pin the verdict.
pub(crate) fn claim_watcher(
    jobs: &mut std::collections::HashMap<String, std::sync::Arc<Job>>,
    pane: &str,
    job: std::sync::Arc<Job>,
) -> bool {
    if jobs.get(pane).is_some_and(|j| !j.is_stopped()) {
        return false;
    }
    jobs.insert(pane.to_string(), job);
    true
}

/// Boot recovery: re-arm watchers for prompts orphaned by a restart so
/// their replies still land. Dead panes and stale entries are dropped.
pub async fn recover_pending(s: &AppState) {
    let entries: Vec<(String, crate::jobs::persist::PendingPrompt)> =
        s.pending.lock().await.clone().into_iter().collect();
    if entries.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for (pane, pp) in entries {
        if !recoverable(pp.started_unix, now) {
            println!("[recover] dropping stale {pane}");
            // Ownership-guarded like the shell/gone paths below: a fresh
            // resubmit racing boot owns the slot — our stale copy must
            // neither report its drop nor wipe the live intent.
            if !s
                .pending_matches(&pane, pp.chat, pp.thread, &pp.prompt)
                .await
            {
                continue;
            }
            // Delivery-gated clear (gone/shell parity below): wiping the
            // intent on a Telegram outage loses the reply forever with no
            // notice. Bounded by `stale_drop_should_clear` (7d cap) so a
            // dead Telegram never retries forever.
            let delivered = report(
                s,
                pp.chat,
                pp.thread,
                &pane,
                "⚠️ dropped: this prompt went stale while the bot was down (>24h) — please resend",
            )
            .await;
            if stale_drop_should_clear(delivered, pp.started_unix, now) {
                s.clear_pending(&pane).await;
            }
            continue;
        }
        // A boot-time herdr blip must not misclassify an agent pane as a
        // shell (shell tail + intent wipe, no re-arm): retry once.
        let agent_alive = get_agent(&s.cfg.socket, &pane).await.is_ok() || {
            tokio::time::sleep(Duration::from_secs(1)).await;
            get_agent(&s.cfg.socket, &pane).await.is_ok()
        };
        if !agent_alive {
            // Fail-open: a pane.list outage must re-arm optimistically, not
            // declare the pane dead and wipe a live prompt's intent.
            match crate::herdr::client::list_panes(&s.cfg.socket).await {
                Err(e) => {
                    println!(
                        "[recover] pane list failed, re-arming {pane}: {}",
                        crate::types::mask_home(&e.to_string())
                    );
                    // Fall through to the re-arm below.
                }
                Ok(panes) => {
                    // Confirm agent absence: after a joint restart agents
                    // register slowly — a present-but-unlisted agent must
                    // re-arm, never take the shell path (shell tail posted
                    // as the reply + intent wiped). Unknown → re-arm.
                    let agents_gone = match list_agents(&s.cfg.socket).await {
                        Ok(agents) => !agents.iter().any(|a| a.pane == pane),
                        Err(_) => false,
                    };
                    if !agents_gone {
                        // Fall through to the re-arm below.
                    } else if panes.contains(&pane) {
                        match crate::herdr::client::read_shell_output(&s.cfg.socket, &pane, 60)
                            .await
                        {
                            Ok(tail) => {
                                let tail = tail.trim().to_string();
                                if tail.is_empty() {
                                    // Still-running shell command with no output
                                    // yet: keep the intent (bounded by the 24h
                                    // stale drop) AND re-arm a settle follower —
                                    // otherwise the reply waits for the next
                                    // restart instead of landing when the
                                    // command finishes. Cancel-aware + bounded
                                    // like the foreground path.
                                    println!("[recover] shell quiet, keeping {pane}");
                                    let (s2, pane2, pp2) = (s.clone(), pane.clone(), pp.clone());
                                    // Ownership check BEFORE the follower: a
                                    // live resubmit racing boot owns the slot
                                    // (its bump would be stolen by ours —
                                    // both settles retiring loses the reply).
                                    if !s2
                                        .pending_matches(&pane2, pp2.chat, pp2.thread, &pp2.prompt)
                                        .await
                                    {
                                        continue;
                                    }
                                    let Some(epoch) = s2.bump_shell_epoch_if_absent(&pane2).await
                                    else {
                                        continue;
                                    };
                                    tokio::spawn(async move {
                                        let before = crate::handlers::shell_common::shell_snapshot(
                                            &s2, &pane2,
                                        )
                                        .await;
                                        crate::handlers::shell_common::settle_report_shell(
                                            &s2,
                                            crate::handlers::shell_common::ShellSettle {
                                                chat: pp2.chat,
                                                thread: pp2.thread,
                                                pane: pane2,
                                                cmd: pp2.prompt,
                                                before,
                                                kb: None,
                                                epoch,
                                            },
                                        )
                                        .await;
                                    });
                                    continue;
                                }
                                // Undelivered notices keep their intent: wiping
                                // it on a Telegram outage loses the reply forever.
                                // Ownership-guarded (quiet-shell parity above): a
                                // resubmit racing boot owns the slot — clearing
                                // our stale copy would wipe its live intent.
                                if !s
                                    .pending_matches(&pane, pp.chat, pp.thread, &pp.prompt)
                                    .await
                                {
                                    continue;
                                }
                                if report(
                                    s,
                                    pp.chat,
                                    pp.thread,
                                    &pane,
                                    &format!("recovered after restart:\n{tail}"),
                                )
                                .await
                                {
                                    s.clear_pending(&pane).await;
                                    println!("[recover] shell recovered {pane}");
                                } else {
                                    println!("[recover] shell notice undelivered, keeping {pane}");
                                }
                                continue;
                            }
                            Err(e) => {
                                // Shell read outage: keep the intent for next boot.
                                println!(
                                    "[recover] shell read failed, keeping {pane}: {}",
                                    crate::types::mask_home(&e.to_string())
                                );
                                continue;
                            }
                        }
                    } else {
                        // Same ownership guard: a resubmit racing boot owns
                        // the slot — reporting our stale copy as "gone"
                        // then wiping its intent loses the reply with a
                        // bogus notice.
                        if !s
                            .pending_matches(&pane, pp.chat, pp.thread, &pp.prompt)
                            .await
                        {
                            continue;
                        }
                        if report(
                            s,
                            pp.chat,
                            pp.thread,
                            &pane,
                            &format!("pane gone before reply arrived [{pane}]"),
                        )
                        .await
                        {
                            println!("[recover] pane gone, dropping {pane}");
                            s.clear_pending(&pane).await;
                        } else {
                            println!("[recover] gone-notice undelivered, keeping {pane}");
                        }
                        continue;
                    }
                }
            }
        }
        // Adaptive (enqueue parity): blocked alt-screen panes reject the
        // output source — a plain read baselines `[]` and the full
        // scrollback arrives as "fresh" (echo/dupe reply).
        let baseline = read_screen_adaptive(&s.cfg.socket, &pane).await;
        let job = Job::new(baseline, pp.chat, pp.thread);
        *job.prompt.lock().await = pp.prompt.clone();
        // The intent file holds one owed prompt: mirror it in the
        // in-memory count or the next failed submit reads owed==0 and
        // retires the watcher + wipes this intent.
        *job.pending.lock().await = 1;
        // Ownership check BEFORE the claim (shell-path parity above): a
        // live resubmit racing boot owns the slot — re-arming our stale
        // copy would serve the wrong dest/prompt and contend books.
        if !s
            .pending_matches(&pane, pp.chat, pp.thread, &pp.prompt)
            .await
        {
            continue;
        }
        // Single acquisition: check + insert under ONE jobs-lock hold.
        // The awaits above released every lock, so a concurrent re-arm
        // could have inserted this pane meanwhile — never run two
        // watchers. Holding one guard across both closes the race.
        if !claim_watcher(&mut *s.jobs.lock().await, &pane, job.clone()) {
            println!("[recover] already watched {pane}, skipping");
            continue;
        }
        // Stagger re-arms: dozens of pendings must not open dozens of
        // event streams + reads against herdr in the same instant.
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::spawn(watch_job(s.clone(), pane.clone(), job));
        println!("[recover] re-armed {pane}");
    }
}

#[cfg(test)]
#[path = "recover_tests.rs"]
mod tests;
