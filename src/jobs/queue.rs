//! Per-pane prompt queue: submissions landing while a turn is owed
//! (or queued items already wait) hold FIFO instead of superseding the
//! live turn — like opencode's own input queue. The watcher serves one
//! item per turn at settle; each gets its own final. RAM-only (durably
//! only the served turn is recorded — `follow` parity: a restart loses
//! queued-but-unserved prompts, never a delivered one). Pruned with
//! pane death in hygiene; dropped on `/cancel` (cancel means cancel).
use crate::{herdr::client::rpc_t, jobs::job::Job, state::AppState};
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};

/// Max held prompts per pane (fail-visible bound, never silent
/// growth: a runaway client refuses loudly instead of queueing forever).
pub(crate) const QUEUE_CAP: usize = 10;

/// One held prompt: everything the serve needs (dest rides along so a
/// queued DM prompt still answers in DM after a topic turn).
#[derive(Clone)]
pub(crate) struct QueuedPrompt {
    pub chat_id: i64,
    pub thread_id: Option<i64>,
    pub text: String,
}

/// Queue length for `pane` (pure lock read, never across RPC).
pub(crate) async fn queue_len(s: &AppState, pane: &str) -> usize {
    s.prompt_queue
        .lock()
        .await
        .get(pane)
        .map(VecDeque::len)
        .unwrap_or(0)
}

/// Push behind the live turn. `Err` when full (caller refuses visibly).
/// Returns the 1-based position for the queued ack.
pub(crate) async fn push_queue(s: &AppState, pane: &str, item: QueuedPrompt) -> Result<usize, ()> {
    let mut m = s.prompt_queue.lock().await;
    let q = m.entry(pane.to_string()).or_default();
    if q.len() >= QUEUE_CAP {
        return Err(());
    }
    q.push_back(item);
    Ok(q.len())
}

/// Queue entry for the enqueue fast path (single source): hold FIFO
/// behind the live turn when anything is owed (pending cover or items
/// already waiting) instead of superseding it — like opencode's own
/// input queue. True when consumed (queued ack or full refuse sent —
/// caller returns). No epoch bump here (a bump would retarget the live
/// turn mid-finalize); the serve publishes when its turn comes. Lock
/// reads only, never nested, never across RPC.
pub async fn hold_if_owed(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    chat_id: i64,
    thread_id: Option<i64>,
    text: String,
) -> bool {
    let owed = *job.pending.lock().await > 0;
    if !owed && queue_len(s, pane).await == 0 {
        return false;
    }
    match push_queue(
        s,
        pane,
        QueuedPrompt {
            chat_id,
            thread_id,
            text,
        },
    )
    .await
    {
        Ok(pos) => {
            s.tg.send_silent(chat_id, thread_id, &crate::ui::queued_ack(pos))
                .await;
        }
        Err(()) => {
            s.tg.send_msg(chat_id, thread_id, crate::ui::QUEUE_FULL, None)
                .await;
        }
    }
    true
}

/// Destructive pop (single popper wins — concurrent servicers never
/// double-serve; the loser sees `None`).
async fn pop_queue(s: &AppState, pane: &str) -> Option<QueuedPrompt> {
    let mut m = s.prompt_queue.lock().await;
    let q = m.get_mut(pane)?;
    let item = q.pop_front()?;
    if q.is_empty() {
        m.remove(pane);
    }
    Some(item)
}

/// Drop the whole queue (cancel paths: a cancelled prompt must never
/// resurface on a later turn).
pub(crate) async fn clear_queue(s: &AppState, pane: &str) {
    s.prompt_queue.lock().await.remove(pane);
}

/// Loop-top serve (runner parity): a live watcher with nothing owed
/// but items held picks the next item up instead of idling it into a
/// strand (fail-path park, or a push landing between the settle serve
/// and retire). The length pre-check keeps idle ticks RPC-free
/// (serve_next fetches status itself). serve_next owns the
/// blocked/detached verdicts (parked, never served); a publish bumps
/// the epoch and the caller's next iteration syncs the new turn.
/// Fresh-race merge with a concurrent submit is the documented
/// concurrent-submit class (last-wins), never a loss: every item is
/// either queued, published, or visibly refused.
pub async fn serve_if_freed(s: &AppState, pane: &str, job: &Arc<Job>) -> bool {
    *job.pending.lock().await == 0 && queue_len(s, pane).await > 0 && serve_next(s, pane, job).await
}

/// Fail-path drain (enqueue entry): serve held prompts when this job
/// still owns the map with nothing owed — but only then (a live turn's
/// settle hook serves them, and a mid-turn publish here would skew its
/// echo boundary; a detached fallback has no watcher to continue into
/// — its items wait for the next submit's turn). Skipped on a stopped
/// job (cancel owns the pane and drained the queue).
pub async fn serve_failed(s: &AppState, pane: &str, job: &Arc<Job>) {
    let mapped = s
        .jobs
        .lock()
        .await
        .get(pane)
        .map(|j| Arc::ptr_eq(j, job))
        .unwrap_or(false);
    if !job.is_stopped() && mapped && *job.pending.lock().await == 0 {
        serve_next(s, pane, job).await;
    }
}

/// Serve held prompts as fresh turns until one publishes, the queue
/// empties, or the agent proves blocked. True when a turn was published
/// (caller keeps watching). Every outcome is terminal per item: served
/// items publish (epoch bump, durable intent, focus, history — enqueue
/// parity), failed items report to their own dest (submitter-always-
/// hears parity), blocked items re-queue at the front and stop (the
/// agent takes nothing while blocked — the answer's follow-up serves
/// them at its turn end). Remap-safe (finalize_blocked parity): a paced
/// reset migrating the topic mid-queue must not buzz the corpse thread.
/// Single owner serves: a detached watcher re-queues instead of
/// publishing into a turn it no longer owns (no double finals).
pub async fn serve_next(s: &AppState, pane: &str, job: &Arc<Job>) -> bool {
    // Ownership + liveness first (fail-closed): cancel owns stopped
    // panes (and drained the queue) — a stopped or detached job never
    // serves the shared queue.
    if job.is_stopped() || !super::progress_retire::is_owner(s, pane, job).await {
        return false;
    }
    // Blocked (or unreadable) agent takes nothing — park without
    // popping (the answer's follow-up serves at its turn end).
    // Fail-closed on RPC error: never submit on ambiguous status.
    match crate::herdr::client::get_agent(&s.cfg.socket, pane).await {
        Ok(a) if a.status != "blocked" => {}
        _ => return false,
    }
    let mut published = false;
    while let Some(q) = pop_queue(s, pane).await {
        // Re-verify per item (a cancel/supersede landing mid-drain must
        // not publish into dead ownership — re-queue and stand down).
        if job.is_stopped() || !super::progress_retire::is_owner(s, pane, job).await {
            s.prompt_queue
                .lock()
                .await
                .entry(pane.to_string())
                .or_default()
                .push_front(q);
            return published;
        }
        // Fresh dest at serve time: the thread may have reminted while
        // the item waited. Unmapped forum dests retire with a visible
        // note at the item's own dest (the queued ack promised a run —
        // log-only would break submitter-always-hears parity; a dead
        // thread fails the send silently, same as the orphan refuse).
        let th = if s.cfg.forum == Some(q.chat_id) {
            match s.topics.storage.get_thread(pane) {
                Some(cur) => Some(cur),
                None => {
                    println!("[jobs] queue {pane}: topic gone, dropping held prompt");
                    super::finalize::report(
                        s,
                        q.chat_id,
                        q.thread_id,
                        pane,
                        &crate::ui::error_card(crate::ui::QUEUE_TOPIC_GONE),
                    )
                    .await;
                    continue;
                }
            }
        } else {
            q.thread_id
        };
        let res = rpc_t(
            &s.cfg.socket,
            "agent.prompt",
            json!({"target": pane, "text": q.text}),
            30,
        )
        .await;
        match res {
            Ok(_) => {
                // Delivered: full last-wins publish (enqueue parity —
                // dest + prompt + generation move together under the
                // pending lock, or a settle snapshot skews the share).
                job.publish_submit(q.chat_id, th, &q.text).await;
                s.set_focus(pane).await;
                s.remember_pending(pane, q.chat_id, th, &q.text).await;
                s.push_history(pane, &q.text).await;
                published = true;
                break;
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.to_lowercase().contains("blocked") {
                    // Blocked: the agent takes nothing right now — park
                    // the item at the front for the answer's follow-up
                    // instead of error-spamming the whole queue.
                    s.prompt_queue
                        .lock()
                        .await
                        .entry(pane.to_string())
                        .or_default()
                        .push_front(q);
                    break;
                }
                // Any other failure belongs to this item alone: report
                // to its dest and serve on (never strand behind it).
                super::finalize::report(s, q.chat_id, th, pane, &crate::ui::error_card(&msg)).await;
            }
        }
    }
    published
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> QueuedPrompt {
        QueuedPrompt {
            chat_id: 7,
            thread_id: None,
            text: text.to_string(),
        }
    }

    #[tokio::test]
    async fn test_queue_is_fifo_and_bounded() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        assert_eq!(queue_len(&s, "w1:p1").await, 0);
        assert_eq!(push_queue(&s, "w1:p1", item("a")).await, Ok(1));
        assert_eq!(push_queue(&s, "w1:p1", item("b")).await, Ok(2));
        // Full: refuses loudly, keeps the old items.
        for i in 0..(QUEUE_CAP - 2) {
            assert!(
                push_queue(&s, "w1:p1", item(&format!("f{i}")))
                    .await
                    .is_ok()
            );
        }
        assert_eq!(push_queue(&s, "w1:p1", item("over")).await, Err(()));
        assert_eq!(queue_len(&s, "w1:p1").await, QUEUE_CAP);
    }

    #[tokio::test]
    async fn test_clear_queue_drops_all() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        push_queue(&s, "w1:p1", item("a")).await.unwrap();
        clear_queue(&s, "w1:p1").await;
        assert_eq!(queue_len(&s, "w1:p1").await, 0);
        // Idempotent on empty.
        clear_queue(&s, "w1:p1").await;
    }
}
