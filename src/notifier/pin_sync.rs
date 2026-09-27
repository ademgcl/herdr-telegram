//! Identity-pin sync for forum topics: split from `status` (500-line
//! file limit). Edits the pane's pin card in place, mints it fresh when
//! definitely gone. Bounded: a flood-wait must not park the sequential
//! reconcile loop — a timeout retries next tick.
use crate::{jobs::report::FINAL_SEND_TIMEOUT_SECS, state::AppState};
use std::time::Duration;

/// Sync the pane's identity pin card (converging rule shared with
/// `jobs::report`): only a definitely-gone card earns a fresh post —
/// a transient failure keeps the pin and retries next tick. Skips
/// unchanged text: status observations arrive every couple of seconds
/// and an unconditional edit per sample is pure write volume (it
/// counted against the same flood budget that delayed the instant
/// message).
pub async fn sync_identity_pin(s: &AppState, pane: &str, forum: i64, card: &str) {
    // Content-addressed: the pin already shows this exact card.
    if s.pinned_card.lock().await.get(pane).map(String::as_str) == Some(card) {
        return;
    }
    // Snapshot (thread, mid) together: a reset migrating the mapping
    // mid-window must not edit the corpse topic successfully and then
    // skip the mint in the new one.
    let thread_at_read = s.topics.all_mappings().get(pane).copied();
    let mut mid_opt = s.topics.get_pin(pane);
    if let Some(mid) = mid_opt {
        let edit = tokio::time::timeout(
            Duration::from_secs(8),
            s.tg.try_edit_msg(forum, mid, card, None),
        )
        .await;
        match edit {
            Ok(Ok(())) => {
                // Remint raced the edit: the corpse edit landed but the
                // mapping moved — treat as gone so the mint branch below
                // posts in the new topic.
                if s.topics.all_mappings().get(pane).copied() != thread_at_read {
                    mid_opt = None;
                }
            }
            Ok(Err(e)) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                mid_opt = None;
            }
            _ => {}
        }
    }
    if mid_opt.is_none()
        && let Some(thread) = s.topics.all_mappings().get(pane).copied()
        // Final-send bound, never shorter than the inner per-call budget:
        // an 8s outer around `send_msg` (15s/call + flood-waits) fires on
        // healthy sends AFTER Telegram posted, losing the message_id —
        // the pin lands untracked and the next tick mints an orphan
        // duplicate. A timeout here still reads as undelivered (retry
        // next tick, same orphan-on-truncate tradeoff as final cards).
        && let Some(new_mid) = tokio::time::timeout(
            Duration::from_secs(FINAL_SEND_TIMEOUT_SECS),
            s.tg.send_msg(forum, Some(thread), card, None),
        )
        .await
        .ok()
        .flatten()
    {
        if !s.topics.set_pin_if_thread(pane, thread, new_mid) {
            // Reminted during send: our just-posted card is the duplicate
            // — delete it so only one pin survives (overwrite-only).
            s.tg.delete_msg(forum, new_mid).await;
            println!("[alert] pin reminted during send for {pane} — dropped duplicate {new_mid}");
            return;
        }
        stamp_pinned(s, pane, card).await;
        return;
    }
    // Edit landed (or the mint was skipped): remember the text so the
    // next unchanged sample costs nothing. Stamped last: a failed sync
    // stays unstamped and the next tick retries it.
    if mid_opt.is_some() {
        stamp_pinned(s, pane, card).await;
    }
}

/// Remember what the pin currently shows (content-addressed gate above).
async fn stamp_pinned(s: &AppState, pane: &str, card: &str) {
    s.pinned_card
        .lock()
        .await
        .insert(pane.to_string(), card.to_string());
}
