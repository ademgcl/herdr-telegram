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

    // Reuse the live watcher; a stopped job is replaced (reusing it
    // attaches the prompt to a dead watcher whose cancel eats it).
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
            let j = Job::new(baseline, chat_id, thread_id);
            // Atomic re-check + claim under ONE lock hold (recover
            // parity): a concurrent enqueue claiming during the baseline
            // read wins — adopt it, never spawn a second watcher
            // (double alerts on every tick).
            let mut map = s.jobs.lock().await;
            if let Some(w) = map.get(&pane).cloned().filter(|x| !x.is_stopped()) {
                w
            } else if super::recover::claim_watcher(&mut map, &pane, j.clone()) {
                drop(map);
                tokio::spawn(watch_job(s.clone(), pane.clone(), j.clone()));
                j
            } else {
                // Lost the atomic race after all: adopt the winner.
                drop(map);
                s.jobs
                    .lock()
                    .await
                    .get(&pane)
                    .cloned()
                    .filter(|x| !x.is_stopped())
                    .unwrap_or(j)
            }
        }
    };

    // Deliver FIRST, record after: a failed submit bumps nothing.
    // Reservation-window cover: light + sustain the indicator while the
    // 30s submit RPC is in flight (spawned, never awaited).
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
        // success landing during the report bumped pending — retiring on
        // a stale owed==0 would wipe its cover + durable intent.
        if *job.pending.lock().await > 0 {
            return;
        }
        // Nothing owed: retire the map entry synchronously so the next
        // enqueue starts clean (never clear the durable slot here: any
        // intent present belongs to a racing shell/corpsed submit).
        // No notify: the watcher exits silently at its loop top.
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
    // Cancel won during the submit RPC: the job is stopped and its
    // durable intent cleared — writing books now would resurrect
    // cancelled work (pending_matches below would match our own rewrite).
    if job.is_stopped() {
        return;
    }
    // Delivered: last-wins books + durable intent. Epoch BEFORE the
    // pending bump (settle_books captures epoch-then-count: count-first
    // overcounts into the newcomer's cover, epoch-first only undercounts).
    *job.dest.lock().await = (req.chat_id, req.message_thread_id);
    *job.prompt.lock().await = req.text.clone();
    job.epoch.fetch_add(1, Ordering::Relaxed);
    *job.pending.lock().await += 1;
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
    // A LIVE successor (concurrent enqueue won mid-submit) needs the same
    // full last-wins transfer — otherwise our prompt is owed to a dead
    // watcher while the successor serves with a short count (dropped
    // reply). Same job + live needs nothing (books already landed home).
    let live_successor = s
        .jobs
        .lock()
        .await
        .get(&pane)
        .cloned()
        .filter(|j| !Arc::ptr_eq(j, &job) && !j.is_stopped());
    if let Some(j) = live_successor {
        super::enqueue_transfer::transfer_live(
            &s,
            &pane,
            &j,
            req.chat_id,
            req.message_thread_id,
            &req.text,
        )
        .await;
        return;
    }
    let rearm = match s.jobs.lock().await.get(&pane).cloned() {
        Some(j) if Arc::ptr_eq(&j, &job) => false,
        Some(j) => j.is_stopped(),
        None => true,
    };
    // Transfer gap cover: a live successor inserted between the two
    // locks above sees rearm==false (live, not stopped) with no transfer
    // — our delivered books strand on a detached Arc (dropped reply).
    // A live non-self job here always takes the full last-wins transfer.
    if !rearm {
        if let Some(j) = s
            .jobs
            .lock()
            .await
            .get(&pane)
            .cloned()
            .filter(|j| !Arc::ptr_eq(j, &job) && !j.is_stopped())
        {
            super::enqueue_transfer::transfer_live(
                &s,
                &pane,
                &j,
                req.chat_id,
                req.message_thread_id,
                &req.text,
            )
            .await;
        }
        return;
    }
    if rearm {
        let baseline = read_screen(&s.cfg.socket, &pane, 400).await;
        // Ownership re-check: a /cancel landing during the submit RPC /
        // baseline read cleared the slot — minting now resurrects it.
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
                    super::enqueue_transfer::transfer_live(
                        &s,
                        &pane,
                        &j,
                        req.chat_id,
                        req.message_thread_id,
                        &req.text,
                    )
                    .await;
                } else {
                    map.insert(pane.clone(), j2.clone());
                    drop(map);
                    println!("[jobs] re-armed watcher for {pane} (retired mid-submit)");
                    tokio::spawn(watch_job(s.clone(), pane.clone(), j2.clone()));
                }
            }
            Some(j) => {
                // (No map held here — live_other was cloned above.)
                super::enqueue_transfer::transfer_live(
                    &s,
                    &pane,
                    &j,
                    req.chat_id,
                    req.message_thread_id,
                    &req.text,
                )
                .await;
            }
        }
    }
}
