//! Delivery helpers for live prompts and reports. Split from finalize.rs.
use crate::state::AppState;

/// User-cancel text: cancel branches post it fresh (no live card exists
/// to edit — see live.rs). Single source so every cancel path buzzes
/// the same card.
pub const CANCELLED: &str = "✋ cancelled";

/// Cancel ownership verdict (pure, tested): a superseding enqueue owns
/// the intent — a stale watcher's retire clears it only when the jobs
/// map still points here AT the entry epoch. Same-Arc reuse bumps the
/// epoch in place (`ptr_eq` alone cannot tell a successor apart — see
/// books.rs), so the epoch pins the generation. Single source for
/// `runner::cancel_watch`.
pub(crate) fn cancel_owns_intent(
    current: Option<&std::sync::Arc<super::job::Job>>,
    job: &std::sync::Arc<super::job::Job>,
    epoch_at_entry: u64,
) -> bool {
    current.is_some_and(|j| {
        std::sync::Arc::ptr_eq(j, job)
            && job.epoch.load(std::sync::atomic::Ordering::Relaxed) == epoch_at_entry
    })
}

pub async fn report(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> bool {
    send_remembered(s, chat_id, thread_id, pane, msg)
        .await
        .is_some()
}

/// Mid-returning final-card part delivery with ✅ reaction: shared by
/// `finalize`'s multi-part post (moved here from `finalize` under the
/// file limit). Returns the mid so `finalize`'s mid-post supersede can
/// delete delivered heads best-effort (stall.rs parity) instead of
/// stranding a stale head beside the new prompt's turn.
pub async fn report_done_mid(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> Option<i64> {
    let m = send_remembered(s, chat_id, thread_id, pane, msg).await?;
    let _ = s.tg.set_reaction(chat_id, m, Some("✅")).await;
    Some(m)
}

/// Bound for final-card sends: `send_msg` sleeps through flood-waits
/// (minutes) — a parked finalize stalls settle + /cancel past the tick.
/// A timeout reads as undelivered (the existing retry path keeps the
/// intent), never as loss. Live edits use the shorter shared bound.
/// Shared with the identity-pin mint (same orphan-on-truncate tradeoff).
pub(crate) const FINAL_SEND_TIMEOUT_SECS: u64 = 90;

/// Send + delivery-track, without any reaction: `report` (plain cards)
/// and `report_done_mid` (✅ finals) share it so failure logging and intent
/// tracking can never drift between the two.
async fn send_remembered(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> Option<i64> {
    let mid = tokio::time::timeout(
        std::time::Duration::from_secs(FINAL_SEND_TIMEOUT_SECS),
        s.tg.send_msg(chat_id, thread_id, msg, None),
    )
    .await
    .ok()
    .flatten();
    if mid.is_none() {
        eprintln!("[prompt] delivery failed {pane} (thread {thread_id:?})");
        return None;
    }
    s.remember(chat_id, mid, pane).await;
    mid
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
