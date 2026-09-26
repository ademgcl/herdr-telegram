//! Post-submit re-arm: mint a watcher when the submit landed on a
//! retired job. Split from `enqueue` (300-line file limit).
use super::runner::watch_job;
use crate::{herdr::client::read_screen_adaptive, jobs::job::Job, state::AppState};
use std::sync::Arc;

/// Re-arm a watcher for delivered-but-watcherless books (see
/// `enqueue_prompt`): ownership re-checked so a /cancel clearing the
/// slot is never resurrected, and a live successor always takes the
/// full last-wins transfer instead of a second watcher.
pub(crate) async fn rearm_watcher(
    s: &AppState,
    pane: &str,
    chat_id: i64,
    thread_id: Option<i64>,
    text: &str,
) {
    // Adaptive (enqueue parity): blocked/working alt-screen panes reject
    // the output source — a plain read baselines `[]` and the full
    // scrollback arrives as "fresh" (echo/dupe reply).
    let baseline = read_screen_adaptive(&s.cfg.socket, pane).await;
    // Ownership re-check: a /cancel landing during the submit RPC /
    // baseline read cleared the slot — minting now resurrects it.
    if !s.pending_matches(pane, chat_id, thread_id, text).await {
        return;
    }
    // A concurrent enqueue may have won while the baseline read yielded
    // — a mapped live job is always a successor: transfer the full
    // last-wins cover instead of spawning a second watcher. Re-check
    // ownership first: a /cancel landing in the same window cleared the
    // slot — transferring now re-mints its durable intent (resurrection).
    if !s.pending_matches(pane, chat_id, thread_id, text).await {
        return;
    }
    let live_other = s
        .jobs
        .lock()
        .await
        .get(pane)
        .cloned()
        .filter(|j| !j.is_stopped());
    match live_other {
        None => {
            let j2 = Job::new(baseline, chat_id, thread_id);
            *j2.prompt.lock().await = text.to_string();
            *j2.pending.lock().await = 1;
            // Re-check under the insert lock: a concurrent enqueue may
            // have won while the field writes above yielded.
            let mut map = s.jobs.lock().await;
            if let Some(j) = map.get(pane).cloned().filter(|j| !j.is_stopped()) {
                drop(map);
                super::enqueue_transfer::transfer_live(s, pane, &j, chat_id, thread_id, text).await;
            } else {
                map.insert(pane.to_string(), j2.clone());
                drop(map);
                // Post-insert ownership re-check: a /cancel clearing the
                // slot between the check above and now must not gain a
                // watcher for dead intent — tear down our own insert.
                if !s.pending_matches(pane, chat_id, thread_id, text).await {
                    let mut map = s.jobs.lock().await;
                    if map.get(pane).map(|j| Arc::ptr_eq(j, &j2)).unwrap_or(false) {
                        map.remove(pane);
                    }
                    return;
                }
                println!("[jobs] re-armed watcher for {pane} (retired mid-submit)");
                tokio::spawn(watch_job(s.clone(), pane.to_string(), j2.clone()));
            }
        }
        Some(j) => {
            // Gap cover (mint-path parity above): a /cancel stopping the
            // successor + clearing the slot between the snapshot and now
            // must not re-mint the durable intent onto a dead watcher.
            if j.is_stopped() || !s.pending_matches(pane, chat_id, thread_id, text).await {
                return;
            }
            super::enqueue_transfer::transfer_live(s, pane, &j, chat_id, thread_id, text).await;
        }
    }
}
