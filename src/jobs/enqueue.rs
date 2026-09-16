//! Prompt submit: deliver to the agent, record last-wins books, arm the
//! watcher. Split from `runner` (300-line file limit).
use super::runner::watch_job;
use crate::{
    handlers::dialog::send_blocked_card,
    herdr::client::{read_screen, rpc_t},
    jobs::finalize::report,
    jobs::job::Job,
    state::AppState,
    types::{AgentRow, PromptRequest},
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;

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

    // Reuse the live watcher when there is one; a stopped (retired) job
    // is replaced — reusing it would attach the prompt to a dead watcher
    // while its cancel branch eats the fresh intent.
    let existing = s
        .jobs
        .lock()
        .await
        .get(&pane)
        .cloned()
        .filter(|j| !j.is_stopped());
    let job = match existing {
        Some(j) => j,
        None => {
            let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
            // Re-check under a fresh lock: a concurrent enqueue may have
            // won the pane while the baseline read yielded — spawning a
            // second watcher would double-report every alert.
            if let Some(j) = s
                .jobs
                .lock()
                .await
                .get(&pane)
                .cloned()
                .filter(|j| !j.is_stopped())
            {
                j
            } else {
                let j = Job::new(baseline, chat_id, thread_id);
                s.jobs.lock().await.insert(pane.clone(), j.clone());
                tokio::spawn(watch_job(s.clone(), pane.clone(), j.clone()));
                j
            }
        }
    };

    // Deliver FIRST, record after: a failed submit must neither bump the
    // epoch (it would reset the live watcher's stream state for nothing)
    // nor overwrite dest/prompt with undelivered text.
    // Reservation-window cover: the 30s submit RPC runs before the
    // watcher exists — light the indicator now. `start_typing` no-ops
    // in DM mode, so touch dest directly + sustain it until submit
    // lands (else a DM submit sits dark up to 30s).
    s.start_typing(&pane).await;
    s.tg.typing(req.chat_id, req.message_thread_id).await;
    let sustain = if s.cfg.forum.is_none() {
        let tg = s.tg.clone();
        let (c, t) = (req.chat_id, req.message_thread_id);
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                tg.typing(c, t).await;
            }
        }))
    } else {
        None
    };
    let submit_res = rpc_t(
        &s.cfg.socket,
        "agent.prompt",
        json!({"target": pane, "text": req.text}),
        30,
    )
    .await;
    if let Some(h) = sustain {
        h.abort();
    }
    if let Err(e) = submit_res {
        println!("[jobs] submit error: {e}");
        // Owed = prior prompts only (this one was never recorded).
        let owed = *job.pending.lock().await;
        // The submitter always hears the truth about their own submit,
        // even when older work stays covered by the running watcher.
        let msg = e.to_string();
        if msg.contains("blocked") {
            send_blocked_card(&s, req.chat_id, req.message_thread_id, &pane).await;
        } else {
            report(
                &s,
                req.chat_id,
                req.message_thread_id,
                &pane,
                &format!("⚠️ error: {e}"),
            )
            .await;
        }
        if owed > 0 {
            // Co-owned prompts remain: watcher and intent stay.
            return;
        }
        // Nothing owed: retire synchronously (stopped jobs are also
        // replaced, never reused) so the next enqueue starts clean.
        // (Without this it finalizes on the untouched screen — the bogus
        // "(no captured output)" card.) No notify: the watcher exits
        // silently at its loop top instead of posting "cancelled" for a
        // retire that was never a user cancel.
        job.mark_stopped();
        let owned = {
            let mut map = s.jobs.lock().await;
            if map
                .get(&pane)
                .map(|j| Arc::ptr_eq(j, &job))
                .unwrap_or(false)
            {
                map.remove(&pane);
                true
            } else {
                false
            }
        };
        // Dropped the jobs guard BEFORE the pending lock + disk write
        // (never nest jobs→pending). Stop our typing now: a parked
        // watcher (5s tick / 60s backoff) would else type into the void
        // until it exits — unless_owned keeps a successor's task.
        if owned {
            s.clear_pending(&pane).await;
        }
        s.stop_typing_unless_owned(&pane).await;
        return;
    }
    // Delivered: last-wins dest/prompt/epoch, durable intent so a restart
    // re-arms this watcher instead of eating the reply.
    *job.dest.lock().await = (req.chat_id, req.message_thread_id);
    *job.prompt.lock().await = req.text.clone();
    *job.pending.lock().await += 1;
    job.epoch.fetch_add(1, Ordering::Relaxed);
    s.set_focus(&pane).await;
    s.remember_pending(&pane, req.chat_id, req.message_thread_id, &req.text)
        .await;
}
