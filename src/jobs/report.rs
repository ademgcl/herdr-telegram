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
