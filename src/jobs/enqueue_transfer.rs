//! Last-wins prompt transfer (split from `enqueue`: 300-line file limit).
//! Single source for the delivered-books handoff: dest/prompt move to
//! the delivered req so they match the durable slot, or a prompt/dest
//! split mismatches `clear_pending_if_matches` and leaks the intent
//! (ghost re-arm after restart). History/focus are recorded once by the
//! caller; the durable rewrite here is idempotent.
use super::job::Job;
use crate::state::AppState;
use std::sync::Arc;

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
    // Atomic publish (enqueue parity — see publish_submit): dest +
    // prompt + generation move together, or a finalize snapshotting
    // between split writes skews the entry share.
    target.publish_submit(chat_id, thread_id, text).await;
    s.remember_pending(pane, chat_id, thread_id, text).await;
}
