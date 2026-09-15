use super::runner::watch_job;
use crate::{
    herdr::client::{get_agent, read_screen},
    jobs::finalize::report,
    jobs::job::Job,
    state::AppState,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Boot recovery: re-arm watchers for prompts orphaned by a restart so
/// their replies still land. Dead panes and >24h-old entries are dropped.
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
        if now.saturating_sub(pp.started_unix) > 86400 || pp.started_unix > now.saturating_add(3600)
        {
            println!("[recover] dropping stale {pane}");
            s.clear_pending(&pane).await;
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
                    println!("[recover] pane list failed, re-arming {pane}: {e}");
                    // Fall through to the re-arm below.
                }
                Ok(panes) if panes.contains(&pane) => {
                    match crate::herdr::client::read_shell_output(&s.cfg.socket, &pane, 60).await {
                        Ok(tail) => {
                            let tail = tail.trim().to_string();
                            if tail.is_empty() {
                                // Still-running shell command with no output
                                // yet: keep the intent (bounded by the 24h
                                // stale drop) so the next boot re-evaluates
                                // instead of eating a live command's reply.
                                println!("[recover] shell quiet, keeping {pane}");
                                continue;
                            }
                            // Undelivered notices keep their intent: wiping
                            // it on a Telegram outage loses the reply forever.
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
                            println!("[recover] shell read failed, keeping {pane}: {e}");
                            continue;
                        }
                    }
                }
                Ok(_) => {
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
        let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
        // Defensive: never run two watchers on one pane (two episodes =
        // double alerts for the same stall). Boot runs once with an empty
        // map, but a re-entrant call must not duplicate.
        if s.jobs.lock().await.contains_key(&pane) {
            println!("[recover] already watched {pane}, skipping");
            continue;
        }
        let job = Job::new(baseline, pp.chat, pp.thread);
        *job.prompt.lock().await = pp.prompt.clone();
        s.jobs.lock().await.insert(pane.clone(), job.clone());
        // Stagger re-arms: dozens of pendings must not open dozens of
        // event streams + reads against herdr in the same instant.
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::spawn(watch_job(s.clone(), pane.clone(), job));
        println!("[recover] re-armed {pane}");
    }
}
