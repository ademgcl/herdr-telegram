use std::sync::Arc;
use serde_json::json;
use tokio::time::{Duration, Instant};
use crate::{
    herdr::client::{get_agent, read_screen, rpc_t},
    jobs::job::Job,
    notifier::{observe_status, refresh_topic_title},
    state::AppState,
    types::{
        AgentRow, PromptRequest, LIVE_EDIT_COOLDOWN_SECS, LIVE_TICK_SECS, MAX_MSG_UNITS,
    },
    ui::{chunks, emoji, tail_fit},
};

/// Terminal statuses that end a watch cycle.
const SETTLED: &[&str] = &["idle", "done", "blocked", "exited", "closed", "dead"];

pub async fn enqueue_prompt(
    s: AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    row: AgentRow,
    text: String,
) {
    let req = PromptRequest {
        chat_id,
        message_thread_id: thread_id,
        text,
    };
    let pane = row.pane.clone();
    println!("[jobs] enqueue for {pane}");

    let existing = s.jobs.lock().await.get(&pane).cloned();
    println!("[jobs] jobs-map checked");
    let job = match existing {
        Some(j) => j,
        None => {
            println!("[jobs] reading baseline…");
            let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
            println!("[jobs] baseline {} lines", baseline.len());
            let j = Job::new(baseline, chat_id, thread_id);
            s.jobs.lock().await.insert(pane.clone(), j.clone());
            tokio::spawn(watch_job(s.clone(), pane.clone(), j.clone()));
            println!("[jobs] watcher spawned");
            j
        }
    };

    *job.dest.lock().await = (req.chat_id, req.message_thread_id);
    *job.pending.lock().await += 1;
    s.set_focus(&pane).await;    println!("[jobs] submitting prompt…");

    // Deliver immediately — interactive agents buffer input like a real terminal
    if let Err(e) = rpc_t(
        &s.cfg.socket,
        "agent.prompt",
        json!({"target": pane, "text": req.text}),
        30,
    )
    .await
    {
        println!("[jobs] submit error: {e}");
        *job.pending.lock().await -= 1;
        report(&s, req.chat_id, req.message_thread_id, &pane, &format!("⚠️ error: {e}")).await;
    }
    println!("[jobs] enqueue done");
}

/// Watch the agent: live-stream fresh output into one editable message,
/// then finalize it into the result card when the agent settles.
async fn watch_job(s: AppState, pane: String, job: Arc<Job>) {
    let mut live_mid: Option<i64> = None;
    let mut last_edit = Instant::now() - Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS);

    loop {
        if job.is_stopped() {
            let (chat, th) = *job.dest.lock().await;
            edit_live(&s, chat, th, &pane, &mut live_mid, "✋ cancelled").await;
            break;
        }

        tokio::select! {
            _ = job.cancel.notified() => {
                job.mark_stopped();
                let (chat, th) = *job.dest.lock().await;
                edit_live(&s, chat, th, &pane, &mut live_mid, "✋ cancelled").await;
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(LIVE_TICK_SECS)) => {
                let Ok(agent) = get_agent(&s.cfg.socket, &pane).await else { continue };

                if SETTLED.contains(&agent.status.as_str()) {
                    // Collapse done↔idle flapping before committing to a report
                    tokio::time::sleep(Duration::from_millis(750)).await;
                    if let Ok(a) = get_agent(&s.cfg.socket, &pane).await
                        && a.status == "working"
                    {
                        continue;
                    }
                    finalize(&s, &pane, &job, &agent.status, &mut live_mid).await;
                    break;
                }

                // Still working — pulse the typing indicator, stream output tail
                let (chat, th) = *job.dest.lock().await;
                s.tg.typing(chat, th).await;
                if last_edit.elapsed() < Duration::from_secs(LIVE_EDIT_COOLDOWN_SECS) {
                    continue;
                }
                let screen = read_screen(&s.cfg.socket, &pane, 400).await;
                let base = job.baseline.lock().await.clone();
                let body = tail_fit(delta(&screen, &base), 3200);
                if body.is_empty() {
                    continue;
                }
                let text = format!("🔄 working…\n\n{body}");
                match live_mid {
                    Some(mid) => s.tg.edit_msg(chat, mid, &text, None).await,
                    None => live_mid = s.tg.send_msg(chat, th, &text, None).await,
                }
                last_edit = Instant::now();
            }
        }
    }

    // Retire only if the map still points at THIS watcher (no newer job took over)
    let mut map = s.jobs.lock().await;
    if map.get(&pane).map(|j| Arc::ptr_eq(j, &job)).unwrap_or(false) {
        map.remove(&pane);
    }
}

/// Turn the live message into the final result card; extra chunks follow it.
async fn finalize(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    live_mid: &mut Option<i64>,
) {
    let screen = read_screen(&s.cfg.socket, pane, 2000).await;
    let base = job.baseline.lock().await.clone();
    let body = join_trimmed(delta(&screen, &base));
    *job.baseline.lock().await = screen;

    let header = format!("{} {settled}", emoji(settled));
    let parts: Vec<String> = if body.is_empty() {
        vec![format!("{header}\n(no new output)")]
    } else {
        chunks(&format!("{header}\n\n{body}"), MAX_MSG_UNITS)
    };

    observe_status(s, pane, settled, true, "job").await;
    let (chat, th) = *job.dest.lock().await;
    for (i, part) in parts.iter().enumerate() {
        match (i, *live_mid) {
            (0, Some(mid)) => s.tg.edit_msg(chat, mid, part, None).await,
            _ => report(s, chat, th, pane, part).await,
        }
    }
    if let Some(th) = th
        && s.cfg.forum == Some(chat)
        && s.topics.pane_of_thread(th).as_deref() == Some(pane)
    {
        s.topics.mark_unread(pane);
    }
    refresh_topic_title(s, pane, settled).await;
    *job.pending.lock().await = 0;

    // Nothing outstanding? Retire the watcher atomically.
    let mut map = s.jobs.lock().await;
    if *job.pending.lock().await == 0
        && map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false)
    {
        map.remove(pane);
        println!("[prompt] watcher retired: {pane}");
    }
}

async fn edit_live(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        s.tg.edit_msg(chat_id, mid, text, None).await;
    } else {
        report(s, chat_id, thread_id, pane, text).await;
    }
}

async fn report(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, msg: &str) {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    s.remember(chat_id, mid, pane).await;
}

/// Output produced after `base` — strips pre-existing scrollback so replies
/// contain only what happened since the last report.
fn delta<'a>(new: &'a [String], base: &[String]) -> &'a [String] {
    if base.is_empty() {
        return new;
    }
    for i in 0..new.len() {
        if new[i..].len() >= base.len() && new[i..i + base.len()] == *base {
            return &new[i + base.len()..];
        }
    }
    // Baseline scrolled off — cut after the newest baseline line still visible
    for b in base.iter().rev() {
        if b.trim().is_empty() {
            continue;
        }
        if let Some(pos) = new.iter().rposition(|l| l == b) {
            return &new[pos + 1..];
        }
    }
    new
}

fn join_trimmed(lines: &[String]) -> String {
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_delta_strips_baseline() {
        let base = v(&["old line 1", "old line 2"]);
        let new = v(&["old line 1", "old line 2", "fresh reply", "done"]);
        assert_eq!(delta(&new, &base), v(&["fresh reply", "done"]));
    }

    #[test]
    fn test_delta_no_baseline() {
        let new = v(&["a", "b"]);
        assert_eq!(delta(&new, &[]), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_delta_scrolled_off_fallback() {
        let base = v(&["marker", "tail"]);
        let new = v(&["junk", "marker", "new stuff"]);
        assert_eq!(delta(&new, &base), v(&["new stuff"]));
    }

    #[test]
    fn test_delta_nothing_new() {
        let base = v(&["same"]);
        let new = v(&["same"]);
        assert!(delta(&new, &base).is_empty());
    }

    #[test]
    fn test_join_trimmed() {
        assert_eq!(join_trimmed(&v(&["", "hi there", ""])), "hi there");
    }
}
