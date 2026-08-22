use std::sync::Arc;
use serde_json::json;
use crate::{
    herdr::client::{read_agent_output, rpc_t},
    jobs::job::Job,
    notifier::{observe_status, refresh_topic_title},
    state::AppState,
    types::{
        AgentRow, PromptRequest, MAX_MSG_UNITS, PROMPT_TIMEOUT_MS, WATCH_REASSURE_ROUNDS,
        WATCH_TIMEOUT_MS,
    },
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
            finish(s, chat_id, thread_id, pane, settled).await;
        }
        Err(e) if e.to_string().contains("timed out") => {
            s.tg.send_msg(chat_id, thread_id, "⏳ still working — pinging when done", None).await;
            watch(s, chat_id, thread_id, pane, job).await;
        }
        Err(e) => {
            report(s, chat_id, thread_id, pane, &format!("⚠️ error: {e}")).await;
        }
    }
}

async fn watch(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, job: &Job) {
    for round in 0u64.. {
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
                        finish(s, chat_id, thread_id, pane, settled).await;
                        return;
                    }
                }
                Err(e) if !e.to_string().contains("timed out") => {
                    report(s, chat_id, thread_id, pane, &format!("⚠️ error: {e}")).await;
                    return;
                }
                Err(_) => {
                    if round % WATCH_REASSURE_ROUNDS == WATCH_REASSURE_ROUNDS - 1 {
                        s.tg.send_msg(chat_id, thread_id, "⏳ still going…", None).await;
                    }
                }
            },
        }
    }
}

async fn finish(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, settled: &str) {
    let out = read_agent_output(&s.cfg.socket, pane, 2000).await.unwrap_or_default();
    let header = format!("{} {settled}", emoji(settled));
    let parts: Vec<String> = if out.is_empty() {
        vec![format!("{header}\n(no readable output)")]
    } else {
        chunks(&format!("{header}\n\n{out}"), MAX_MSG_UNITS)
    };
    observe_status(s, pane, settled, true, "job").await;
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
