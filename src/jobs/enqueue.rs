//! Prompt submit: deliver to the agent, record last-wins books, arm the
//! watcher. Split from `runner` (300-line file limit).
use super::runner::watch_job;
use crate::{
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
    // Reservation-window cover: the watcher spawned above samples the
    // agent while the 30s submit RPC is still in flight — light the
    // indicator now and sustain it until submit lands (else the window
    // sits dark). Unconditional: DM has no typing task at all, and a
    // forum pane still unmapped pauses its task — both need the loop;
    // where the task runs too the touches are idempotent refreshes.
    // Spawned, never awaited.
    s.start_typing(&pane).await;
    {
        let tg = s.tg.clone();
        let (c, t) = (req.chat_id, req.message_thread_id);
        tokio::spawn(async move {
            tg.typing(c, t).await;
        });
    }
    let tg = s.tg.clone();
    let (c, t) = (req.chat_id, req.message_thread_id);
    let sustain = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(
                crate::state::TYPING_TICK_SECS,
            ))
            .await;
            tg.typing(c, t).await;
        }
    });
    let submit_res = rpc_t(
        &s.cfg.socket,
        "agent.prompt",
        json!({"target": pane, "text": req.text}),
        30,
    )
    .await;
    sustain.abort();
    if let Err(e) = submit_res {
        println!(
            "[jobs] submit error: {}",
            crate::types::mask_home(&e.to_string())
        );
        // The submitter always hears the truth about their own submit,
        // even when older work stays covered by the running watcher.
        let msg = e.to_string();
        if msg.contains("blocked") {
            super::enqueue_blocked::report_blocked_submit(
                &s,
                req.chat_id,
                req.message_thread_id,
                &pane,
                &e.to_string(),
            )
            .await;
        } else {
            report(
                &s,
                req.chat_id,
                req.message_thread_id,
                &pane,
                &format!("⚠️ error: {}", crate::types::mask_home(&e.to_string())),
            )
            .await;
        }
        // Live re-read (never the pre-report snapshot): a concurrent
        // successful submit landing during the report sends above bumped
        // pending — retiring on a stale owed==0 would wipe its cover +
        // durable intent (silent prompt loss). This one was never
        // recorded, so live pending is prior prompts only.
        if *job.pending.lock().await > 0 {
            // Co-owned prompts remain: watcher and intent stay.
            return;
        }
        // Nothing owed: retire the map entry synchronously (stopped jobs
        // are also replaced, never reused) so the next enqueue starts
        // clean. (Without this it finalizes on the untouched screen —
        // the bogus "(no captured output)" card.) Never clear the
        // durable slot here: this submit recorded nothing, so any intent
        // present belongs to a racing shell command (shared slot) or a
        // stale corpse (bounded by the 24h reap) — wiping it eats a
        // foreign reply. No notify: the watcher exits silently at its
        // loop top instead of posting "cancelled" for a retire that was
        // never a user cancel.
        job.mark_stopped();
        {
            let mut map = s.jobs.lock().await;
            if map
                .get(&pane)
                .map(|j| Arc::ptr_eq(j, &job))
                .unwrap_or(false)
            {
                map.remove(&pane);
            }
        }
        // Stop our typing now: a parked watcher (5s tick / 60s backoff)
        // would else type into the void until it exits — shell-aware (a
        // racing shell submit's pending keeps its task; an agent
        // successor re-mints via jobs).
        s.stop_shell_typing(&pane).await;
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
    s.push_history(&pane, &req.text).await;
    // Early-retire race: the watcher above may have settled the
    // pre-submit idle screen (5s same-kind persistence) and retired
    // while the submit RPC was in flight — the books just landed on a
    // detached job and the durable intent would sit watcherless until
    // a restart. If the map no longer holds this job and no live
    // successor took the pane, re-arm a watcher for the recorded intent.
    let rearm = match s.jobs.lock().await.get(&pane).cloned() {
        Some(j) if Arc::ptr_eq(&j, &job) => false,
        Some(j) => j.is_stopped(),
        None => true,
    };
    if rearm {
        let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
        // Ownership re-check: a /cancel (or quiet retire) landing during
        // the submit RPC / baseline read cleared the durable slot — minting
        // a watcher now would resurrect cancelled work with in-memory
        // cover but no (or a foreign) intent. Cancel won: drop instead.
        if !s
            .pending_matches(&pane, req.chat_id, req.message_thread_id, &req.text)
            .await
        {
            return;
        }
        // Re-check under a fresh lock: a concurrent enqueue may have won
        // while the baseline read yielded (same pattern as the spawn
        // above) — a mapped live job is always a successor, leave it.
        // Adopt it instead of dropping our prompt: our books already
        // landed on the retired job and durable holds our text, so
        // transfer the full last-wins cover (dest/prompt/pending/epoch)
        // to the live successor — otherwise our prompt is owed to a dead
        // watcher while the successor serves with a short count (dropped
        // reply), or prompt/dest split from durable and leak the intent
        // (ghost re-arm after restart). Lock-free handoff (clone the Arc,
        // drop the map, then write — never nest jobs→pending locks, not
        // even a fresh Job's fields while holding the map).
        let live_other = s
            .jobs
            .lock()
            .await
            .get(&pane)
            .cloned()
            .filter(|j| !j.is_stopped());
        match live_other {
            None => {
                let j2 = Job::new(baseline, req.chat_id, req.message_thread_id);
                *j2.prompt.lock().await = req.text.clone();
                *j2.pending.lock().await = 1;
                // Re-check under the insert lock: a concurrent enqueue
                // may have won while the field writes above yielded —
                // a mapped live job is always a successor, adopt it.
                let mut map = s.jobs.lock().await;
                if let Some(j) = map.get(&pane).cloned().filter(|j| !j.is_stopped()) {
                    drop(map);
                    *j.dest.lock().await = (req.chat_id, req.message_thread_id);
                    *j.prompt.lock().await = req.text.clone();
                    *j.pending.lock().await += 1;
                    j.epoch.fetch_add(1, Ordering::Relaxed);
                    s.remember_pending(&pane, req.chat_id, req.message_thread_id, &req.text)
                        .await;
                } else {
                    map.insert(pane.clone(), j2.clone());
                    drop(map);
                    println!("[jobs] re-armed watcher for {pane} (retired mid-submit)");
                    tokio::spawn(watch_job(s.clone(), pane.clone(), j2.clone()));
                }
            }
            Some(j) => {
                // Full last-wins transfer (house rule, same as the
                // delivered-books path above): dest/prompt move to our req
                // so they match the durable slot our delivered path just
                // wrote — a prompt/dest split would mismatch
                // clear_pending_if_matches and leak the intent (ghost
                // re-arm after restart). History/focus already recorded
                // above for the same req; durable rewrite is idempotent.
                // (No map held here — live_other was cloned above.)
                *j.dest.lock().await = (req.chat_id, req.message_thread_id);
                *j.prompt.lock().await = req.text.clone();
                *j.pending.lock().await += 1;
                j.epoch.fetch_add(1, Ordering::Relaxed);
                s.remember_pending(&pane, req.chat_id, req.message_thread_id, &req.text)
                    .await;
            }
        }
    }
}
