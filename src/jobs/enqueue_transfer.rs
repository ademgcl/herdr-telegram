//! Last-wins prompt transfer (split from `enqueue`: 300-line file limit).
//! Single source for the delivered-books handoff: dest/prompt move to
//! the delivered req so they match the durable slot, or a prompt/dest
//! split mismatches `clear_pending_if_matches` and leaks the intent
//! (ghost re-arm after restart). History/focus are recorded once by the
//! caller; the durable rewrite here is idempotent.
use super::job::Job;
use crate::state::AppState;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Move full last-wins cover (dest/prompt/pending/epoch + durable) for
/// `pane` onto `target`.
pub async fn transfer_live(
    s: &AppState,
    pane: &str,
    target: &Arc<Job>,
    chat_id: i64,
    thread_id: Option<i64>,
    text: &str,
) {
    *target.dest.lock().await = (chat_id, thread_id);
    *target.prompt.lock().await = text.to_string();
    *target.pending.lock().await += 1;
    target.epoch.fetch_add(1, Ordering::Relaxed);
    s.remember_pending(pane, chat_id, thread_id, text).await;
}
