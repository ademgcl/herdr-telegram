use std::sync::Arc;
use serde_json::json;
use crate::{
    herdr::client::{list_agents, list_workspaces, read_agent_output, rpc_t},
    jobs::job::Job,
    notifier::status::observe_status,
    state::AppState,
    types::{AgentRow, PromptRequest, PROMPT_TIMEOUT_MS},
    ui::{agents_summary, emoji},
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
        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
        let summary = agents_summary(&spaces, &agents, &row.pane);
        let msg = format!("📨 queued — runs after current task\n\n{summary}");
        let mid = s.tg.send_msg(chat_id, thread_id, &msg, None).await;
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

        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
        let summary = agents_summary(&spaces, &agents, &pane);
        let notify = format!("📨 got it — {} is on it\n\n{summary}", row.kind);
        let mid = s.tg.send_msg(req.chat_id, req.message_thread_id, &notify, None).await;
        s.remember(req.chat_id, mid, &pane).await;
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

    tokio::select! {
        _ = job.cancel.notified() => {
            job.mark_stopped();
            let mid = s.tg.send_msg(chat_id, thread_id, "✋ prompt cancelled", None).await;
            s.remember(chat_id, mid, pane).await;
        }
        r = fut => {
            let wait_timed_out = match &r {
                Err(e) => e.to_string().contains("agent.prompt timed out"),
                Ok(_) => false,
            };
            let mut msg = if let Err(e) = &r {
                if wait_timed_out { String::new() } else { format!("⚠️ error: {e}") }
            } else {
                String::new()
            };

            if wait_timed_out {
                s.tg.send_msg(chat_id, thread_id, "⏳ still working — pinging when done", None).await;
                for round in 0..12 {
                    if job.is_stopped() { return; }
                    let w = rpc_t(
                        &s.cfg.socket,
                        "agent.wait",
                        json!({"target": pane, "until": ["idle", "done", "blocked"], "timeout_ms": 600_000}),
                        660,
                    ).await;

                    let settled = match &w {
                        Ok(v) => v["agent"]["agent_status"].as_str().or(v["agent_status"].as_str()).map(|x| x.to_string()),
                        Err(e) => {
                            if e.to_string().contains("agent.wait timed out") {
                                if round % 3 == 2 {
                                    s.tg.send_msg(chat_id, thread_id, "⏳ still going…", None).await;
                                }
                                continue;
                            }
                            Some("unknown".into())
                        }
                    };
                    let Some(settled) = settled else { continue };
                    let out = read_agent_output(&s.cfg.socket, pane, 50).await.unwrap_or_default();
                    msg = if out.is_empty() {
                        format!("{} {settled}\n(no readable output)", emoji(&settled))
                    } else {
                        format!("{} {settled}\n\n{out}", emoji(&settled))
                    };
                    observe_status(s, pane, &settled, true, "job").await;
                    break;
                }
                if msg.is_empty() {
                    msg = "⚠️ gave up watching (agent never settled)".into();
                }
            } else if let Ok(v) = &r {
                let settled = v["agent"]["agent_status"].as_str().or(v["agent_status"].as_str()).unwrap_or("unknown");
                let out = read_agent_output(&s.cfg.socket, pane, 50).await.unwrap_or_default();
                msg = if out.is_empty() {
                    format!("{} {settled}\n(no readable output)", emoji(settled))
                } else {
                    format!("{} {settled}\n\n{out}", emoji(settled))
                };
                observe_status(s, pane, settled, true, "job").await;
            }

            let mid = s.tg.send_msg(chat_id, thread_id, &msg, None).await;
            s.remember(chat_id, mid, pane).await;
            s.set_focus(pane).await;
        }
    }
}
