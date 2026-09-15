use std::time::{SystemTime, UNIX_EPOCH};
use super::runner::watch_job;
use crate::{
    herdr::client::{get_agent, read_screen},
    jobs::finalize::report,
    jobs::job::Job,
    state::AppState,
};

/// Boot recovery: re-arm watchers for prompts orphaned by a restart so
/// their replies still land. Dead panes and >24h-old entries are dropped.
pub async fn recover_pending(s: &AppState) {
    let entries: Vec<(String, crate::jobs::persist::PendingPrompt)> =
        s.pending.lock().await.clone().into_iter().collect();
    if entries.is_empty() {
        return;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    for (pane, pp) in entries {
        if now.saturating_sub(pp.started_unix) > 86400 {
            println!("[recover] dropping stale {pane}");
            s.clear_pending(&pane).await;
            continue;
        }
        if get_agent(&s.cfg.socket, &pane).await.is_err() {
            let alive = crate::herdr::client::list_panes(&s.cfg.socket)
                .await
                .unwrap_or_default()
                .contains(&pane);
            if alive {
                if let Ok(tail) = crate::herdr::client::read_shell_output(&s.cfg.socket, &pane, 60).await {
                    let tail = tail.trim().to_string();
                    if !tail.is_empty() {
                        report(s, pp.chat, pp.thread, &pane, &format!("recovered after restart:\n{tail}")).await;
                    }
                }
                s.clear_pending(&pane).await;
                println!("[recover] shell recovered {pane}");
                continue;
            }
            report(s, pp.chat, pp.thread, &pane, &format!("pane gone before reply arrived [{pane}]")).await;
            println!("[recover] pane gone, dropping {pane}");
            s.clear_pending(&pane).await;
            continue;
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
        tokio::spawn(watch_job(s.clone(), pane.clone(), job));
        println!("[recover] re-armed {pane}");
    }
}
