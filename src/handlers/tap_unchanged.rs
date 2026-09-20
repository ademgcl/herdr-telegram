use super::tap_classify::dialog_moved;
use super::tap_keys::TapSend;
use super::tap_refresh::delayed_refresh;
use crate::{handlers::dialog::parse_options, state::AppState};

/// Unchanged-tap outcome: the keys landed but herdr still samples
/// `blocked` with the same dialog on screen. Posts the explainer, then
/// speculatively follows when the screen moved off the dialog (a resume
/// masked by status lag — the lost-final race); still-blocked stands
/// down inside `follow_answer`, so only vanished turns gain a watcher.
#[allow(clippy::too_many_arguments)]
pub async fn handle_unchanged(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
    send: &TapSend,
    before: &[String],
    after: &[String],
    op: crate::state::OpGuard<'_>,
) {
    let mut shown = send.nav.clone();
    shown.extend(send.confirm.iter().cloned());
    let sent = shown.join("+");
    let before_opts = parse_options(before);
    let text = if before_opts.is_empty() {
        format!("⌨️ sent {sent} — check the pane")
    } else {
        format!(
            "⚠️ sent {sent} but the question is still up — /card for fresh buttons, /esc to dismiss, or answer on the PC"
        )
    };
    let mid = s.tg.send_msg(chat, thread, &text, None).await;
    s.remember(chat, mid, pane).await;
    s.tg.strip_buttons(chat, msg_id).await;
    if dialog_moved(before, after) {
        s.blocked_sig.lock().await.remove(pane);
        drop(op);
        crate::jobs::follow::follow_answer(s, pane, chat, thread, &send.label).await;
    }
    delayed_refresh(s, pane).await;
}
