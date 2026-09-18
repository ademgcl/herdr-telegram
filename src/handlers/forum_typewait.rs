//! Agent-topic typed-answer waiter: split from `forum_topic` (300-line cap).
use crate::state::AppState;

/// Waiter outcome: handled (caller returns), resumed-as-prompt (caller
/// enqueues text as a prompt, never control), or pass-through.
pub(crate) enum WaitOut {
    Handled,
    ResumedPrompt,
    Pass,
}

/// An armed typed-answer waiter wins over every command except the
/// escapes checked before (like /cancel, /card, /esc). Peek-first: a
/// blockop race or failed send keeps the waiter; a resume race falls
/// through — a "/" answer becomes a prompt, never control.
pub(crate) async fn consume_typewait(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    text: &str,
    is_cmd: bool,
) -> WaitOut {
    let Some(wpane) = s
        .typewait
        .lock()
        .await
        .get(&(chat, Some(thread_id)))
        .map(|(p, _)| p.clone())
    else {
        return WaitOut::Pass;
    };
    if s.block_held(&wpane).await {
        s.tg
            .send_msg(chat, Some(thread_id), crate::ui::ANSWER_IN_FLIGHT, None)
            .await;
        return WaitOut::Handled;
    }
    match super::tap::type_text(s, &wpane, text).await {
        Ok(()) => {
            s.typewait.lock().await.remove(&(chat, Some(thread_id)));
            s.tg
                .send_msg(chat, Some(thread_id), &crate::ui::typed_ack(&wpane), None)
                .await;
            WaitOut::Handled
        }
        Err(super::tap::TypeError::Resumed) => {
            s.typewait.lock().await.remove(&(chat, Some(thread_id)));
            if is_cmd {
                WaitOut::ResumedPrompt
            } else {
                WaitOut::Pass
            }
        }
        Err(e) => {
            s.tg
                .send_msg(
                    chat,
                    Some(thread_id),
                    &format!("⚠️ type failed: {e} — retry, or /cancel to abort"),
                    None,
                )
                .await;
            WaitOut::Handled
        }
    }
}
