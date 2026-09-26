use crate::{
    handlers::dialog::{DIALOG_READ_LINES, dialog_sig, send_blocked_card},
    herdr::client::read_screen_visible,
    jobs::finalize::report,
    state::{AppState, OpGuard},
    ui::error_card,
};

/// Fresh-card arm verdict (pure, tested): a delivered card is the
/// submitter's answer; a dropped/blank card must still hear the masked
/// error — silence strands them (enqueue's submitter-always-hears
/// contract). `None` = card delivered, stop here.
pub(crate) fn blocked_submit_fallback(card_posted: bool, err: &str) -> Option<String> {
    if card_posted {
        None
    } else {
        Some(error_card(err))
    }
}

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
        report(s, chat_id, thread_id, pane, &error_card(err)).await;
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
            // Consume the verdict: a dropped/blank card still answers
            // the submitter — never silence (the discarded bool left
            // them hanging on the failed submit).
            let posted = send_blocked_card(s, chat_id, thread_id, pane).await;
            if let Some(fallback) = blocked_submit_fallback(posted, err) {
                report(s, chat_id, thread_id, pane, &fallback).await;
            }
        }
    } else {
        report(s, chat_id, thread_id, pane, crate::ui::BLOCKED_SEE_CARD).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocked_submit_fallback_never_silent() {
        // Delivered card ends the exchange.
        assert_eq!(blocked_submit_fallback(true, "boom"), None);
        // Dropped card still answers with the single-source error text.
        let fb = blocked_submit_fallback(false, "herdr boom").expect("fallback");
        assert!(fb.starts_with("⚠️ error:"));
        assert!(fb.contains("herdr boom"));
    }

    #[test]
    fn test_blocked_submit_consumes_card_verdict() {
        // Structural pin: the fresh-card arm must consume
        // send_blocked_card's bool — a discarded `.await;` strands the
        // submitter with silence when the second read is blank.
        let src = include_str!("enqueue_blocked.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        assert!(
            body.contains("let posted = send_blocked_card"),
            "send_blocked_card verdict must be bound, not discarded"
        );
        assert!(
            body.contains("blocked_submit_fallback"),
            "fresh-card arm must route through the fallback"
        );
    }
}
