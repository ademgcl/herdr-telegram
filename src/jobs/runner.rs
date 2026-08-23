use std::sync::Arc;
use serde_json::json;
use crate::{
    herdr::client::{read_agent_output, rpc_t},
    jobs::job::Job,
    notifier::{observe_status, refresh_topic_title},
    state::AppState,
    types::{AgentRow, PromptRequest, MAX_MSG_UNITS, PROMPT_TIMEOUT_MS, WATCH_TIMEOUT_MS},
    ui::{chunks, emoji},
};

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

    let existing = s.jobs.lock().await.get(&row.pane).cloned();
    if let Some(job) = existing {
        job.queue.lock().await.push_back(req);
        let mid = s.tg.send_msg(chat_id, thread_id, "📨 queued — runs after the current task", None).await;
        s.remember(chat_id, mid, &row.pane).await;
        return;
    }

    let job = Job::new();
    s.jobs.lock().await.insert(row.pane.clone(), job.clone());
    tokio::spawn(run_job(s.clone(), req, row, job));
}

async fn run_job(s: AppState, first_req: PromptRequest, row: AgentRow, job: Arc<Job>) {
    let pane = row.pane.clone();
    let mut current = Some(first_req);

    loop {
        let req = match current.take() {
            Some(r) => r,
            None => match job.queue.lock().await.pop_front() {
                Some(r) => r,
                None => break,
            },
        };

        // No chat ack — the agent's topic title flips to 🔄 instead
        refresh_topic_title(&s, &pane, "working").await;
        s.set_focus(&pane).await;

        process_prompt(&s, req.chat_id, req.message_thread_id, &pane, req.text, &job).await;
        if job.is_stopped() {
            break;
        }
    }

    println!("[prompt] job finished: {pane}");
    s.jobs.lock().await.remove(&pane);
}

async fn process_prompt(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    text: String,
    job: &Job,
) {
    println!("[prompt] -> {pane}: {}", text.chars().take(80).collect::<String>());
    let baseline = pane_tail(s, pane, 400).await;
    let fut = rpc_t(
        &s.cfg.socket,
        "agent.prompt",
        json!({
            "target": pane,
            "text": text,
            "wait": {"until": ["idle", "done", "blocked"], "timeout_ms": PROMPT_TIMEOUT_MS}
        }),
        PROMPT_TIMEOUT_MS / 1000 + 60,
    );

    let r = tokio::select! {
        _ = job.cancel.notified() => {
            job.mark_stopped();
            report(s, chat_id, thread_id, pane, "✋ prompt cancelled").await;
            return;
        }
        r = fut => r,
    };

    match r {
        Ok(v) => {
            let settled = v["agent"]["agent_status"]
                .as_str()
                .or(v["agent_status"].as_str())
                .unwrap_or("unknown");
            finish(s, chat_id, thread_id, pane, settled, &baseline).await;
        }
        // Agent still busy — covers BOTH our client-side timeout ("herdr agent.prompt
        // timed out") and herdr's server-side wait timeout ("timed out waiting for
        // agent status"). Stay silent; keep watching until it settles or dies.
        Err(e) if e.to_string().contains("timed out") => {
            watch(s, chat_id, thread_id, pane, job, &baseline).await;
        }
        Err(e) => {
            report(s, chat_id, thread_id, pane, &format!("⚠️ error: {e}")).await;
        }
    }
}

/// Watch an agent until it settles — silently, for as long as it takes.
/// Only speaks up on completion, cancellation, or a real failure.
async fn watch(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    job: &Job,
    baseline: &[String],
) {
    loop {
        if job.is_stopped() { return; }
        let w = rpc_t(
            &s.cfg.socket,
            "agent.wait",
            json!({"target": pane, "until": ["idle", "done", "blocked"], "timeout_ms": WATCH_TIMEOUT_MS}),
            WATCH_TIMEOUT_MS / 1000 + 60,
        );
        tokio::select! {
            _ = job.cancel.notified() => {
                job.mark_stopped();
                report(s, chat_id, thread_id, pane, "✋ prompt cancelled").await;
                return;
            }
            w = w => match w {
                Ok(v) => {
                    let settled = v["agent"]["agent_status"]
                        .as_str()
                        .or(v["agent_status"].as_str())
                        .unwrap_or("unknown");
                    if matches!(settled, "idle" | "done" | "blocked" | "exited" | "closed" | "dead") {
                        finish(s, chat_id, thread_id, pane, settled, &baseline).await;
                        return;
                    }
                }
                Err(e) if !e.to_string().contains("timed out") => {
                    report(s, chat_id, thread_id, pane, &format!("⚠️ error: {e}")).await;
                    return;
                }
                // Round elapsed and the agent is still working — stay quiet
                Err(_) => {}
            },
        }
    }
}

async fn pane_tail(s: &AppState, pane: &str, lines: u32) -> Vec<String> {
    let out = read_agent_output(&s.cfg.socket, pane, lines).await.unwrap_or_default();
    out.lines().map(|l| l.trim_end().to_string()).collect()
}

/// Output produced after `base` — strips pre-existing scrollback so replies
/// contain only what happened since the prompt was sent.
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

async fn finish(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    settled: &str,
    baseline: &[String],
) {
    let screen = pane_tail(s, pane, 2000).await;
    let body = delta(&screen, baseline).join("\n");
    let body = body.trim();
    let header = format!("{} {settled}", emoji(settled));
    let parts: Vec<String> = if body.is_empty() {
        vec![format!("{header}\n(no new output)")]
    } else {
        chunks(&format!("{header}\n\n{body}"), MAX_MSG_UNITS)
    };
    observe_status(s, pane, settled, true, "job").await;
    // Result landed in the agent's own topic → mark unread (marker shows in title)
    if let Some(th) = thread_id
        && s.cfg.forum == Some(chat_id)
        && s.topics.pane_of_thread(th).as_deref() == Some(pane)
    {
        s.topics.mark_unread(pane);
    }
    refresh_topic_title(s, pane, settled).await;
    for part in &parts {
        report(s, chat_id, thread_id, pane, part).await;
    }
    s.set_focus(pane).await;
}

async fn report(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, msg: &str) {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    s.remember(chat_id, mid, pane).await;
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
        // baseline no longer contiguous; last meaningful line still visible
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
}
