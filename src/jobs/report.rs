//! Delivery helpers for live prompts and reports. Split from finalize.rs.
use crate::state::AppState;

pub async fn edit_live(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        if s.tg.try_edit_msg(chat_id, mid, text, None).await.is_err() {
            report(s, chat_id, thread_id, pane, text).await;
        }
    } else {
        report(s, chat_id, thread_id, pane, text).await;
    }
}

pub async fn report(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> bool {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    if mid.is_none() {
        eprintln!("[prompt] delivery failed {pane} (thread {thread_id:?})");
        return false;
    }
    s.remember(chat_id, mid, pane).await;
    true
}

/// Best-effort fold of a live card with NO fallback post: quiet retire
/// paths (dead pane / shell flip) must never buzz a new message — the
/// frozen "working…" card just resolves in place, or stays if the
/// thread is already gone (edit fails silently).
pub async fn fold_live(
    s: &AppState,
    live_dest: &mut Option<(i64, Option<i64>)>,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        if let Some((chat, _)) = live_dest.take() {
            let _ = s.tg.try_edit_msg(chat, mid, text, None).await;
        }
    } else {
        live_dest.take();
    }
}
