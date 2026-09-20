use crate::{
    handlers::dialog::{DIALOG_READ_LINES, dialog_sig, send_blocked_card},
    herdr::client::read_screen_visible,
    jobs::finalize::report,
    state::{AppState, OpGuard},
};

/// Blocked-submit report: the submit failed because the agent is blocked.
/// Gated post (refresh/finalize parity): a tap/finalize in flight owns
/// the card — contention reports plainly without buzz. A sig re-check
/// after the claim stops a second ❗ when a racing post just stamped
/// this exact dialog (both FIRE effects otherwise — the
/// submit-failed-because-blocked interleaving is the common case).
pub async fn report_blocked_submit(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    err: &str,
) {
    let screen = read_screen_visible(&s.cfg.socket, pane, DIALOG_READ_LINES).await;
    if screen.is_empty() {
        report(
            s,
            chat_id,
            thread_id,
            pane,
            &format!("⚠️ error: {}", crate::types::mask_home(err)),
        )
        .await;
        return;
    }
    let sig = dialog_sig(&screen);
    let dup = s
        .blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == &sig)
        .unwrap_or(false);
    if dup || s.block_held(pane).await {
        report(s, chat_id, thread_id, pane, crate::ui::BLOCKED_SEE_CARD).await;
    } else if let Some(_op) = OpGuard::claim(&s.blockop, pane).await {
        let dup2 = s
            .blocked_sig
            .lock()
            .await
            .get(pane)
            .map(|v| v == &sig)
            .unwrap_or(false);
        if dup2 {
            report(s, chat_id, thread_id, pane, crate::ui::BLOCKED_SEE_CARD).await;
        } else {
            send_blocked_card(s, chat_id, thread_id, pane).await;
        }
    } else {
        report(s, chat_id, thread_id, pane, crate::ui::BLOCKED_SEE_CARD).await;
    }
}
