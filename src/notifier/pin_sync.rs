//! Identity-pin sync for forum topics: split from `status` (300-line
//! file limit). Edits the pane's pin card in place, mints it fresh when
//! definitely gone. Bounded: a flood-wait must not park the sequential
//! reconcile loop — a timeout retries next tick.
use crate::state::AppState;
use std::time::Duration;

/// Sync the pane's identity pin card (converging rule shared with
/// `jobs::report`): only a definitely-gone card earns a fresh post —
/// a transient failure keeps the pin and retries next tick.
pub async fn sync_identity_pin(s: &AppState, pane: &str, forum: i64, card: &str) {
    let mut mid_opt = s.topics.get_pin(pane);
    if let Some(mid) = mid_opt {
        let edit = tokio::time::timeout(
            Duration::from_secs(20),
            s.tg.try_edit_msg(forum, mid, card, None),
        )
        .await;
        match edit {
            Ok(Ok(())) => {}
            Ok(Err(e)) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                mid_opt = None;
            }
            _ => {}
        }
    }
    if mid_opt.is_none()
        && let Some(thread) = s.topics.all_mappings().get(pane).copied()
        && let Some(new_mid) = tokio::time::timeout(
            Duration::from_secs(20),
            s.tg.send_msg(forum, Some(thread), card, None),
        )
        .await
        .ok()
        .flatten()
        && !s.topics.set_pin_if_thread(pane, thread, new_mid)
    {
        println!("[alert] pin reminted during send for {pane} — dropping stale mid");
    }
}
