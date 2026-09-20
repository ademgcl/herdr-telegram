//! Stale/unknown button-tap refresh: split from `tap_answer` (300-line
//! file limit). Re-renders the tapped card in place with the live option
//! set; outage/moved-on paths strip + heal instead of stranding buttons.
use super::tap_refresh::delayed_refresh;
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, dialog_sig, live_card},
    herdr::client::{get_agent, read_screen_visible},
    state::AppState,
};
use serde_json::Value;

/// Refresh a turned-over/narrowed dialog card in place. Every path
/// converges via edit/strip + the delayed heal — no stranded buttons.
pub(crate) async fn handle_unknown(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
) {
    // A stale tap on a LIVE pane must strip, never ghost: posting blocked
    // buttons for working output invites blind taps. Fail-closed: an
    // unreadable status posts no buttons and no buzz.
    let live_blocked = match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => a.status == "blocked",
        Err(_) => {
            s.tg.strip_buttons(chat, msg_id).await;
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            delayed_refresh(s, pane).await;
            return;
        }
    };
    if !live_blocked {
        s.blocked_sig.lock().await.remove(pane);
        let no_kb = Some(Value::Array(Vec::new()));
        s.tg.edit_msg(
            chat,
            msg_id,
            &format!("↩️ already moved on [{pane}] — buttons removed"),
            no_kb,
        )
        .await;
        s.remember(chat, Some(msg_id), pane).await;
        // Sibling surfaces may still show this dead dialog with live
        // buttons — the heal below resolves them.
        delayed_refresh(s, pane).await;
        return;
    }
    let screen = read_screen_visible(
        &s.cfg.socket,
        pane,
        crate::handlers::dialog::DIALOG_READ_LINES,
    )
    .await;
    if screen.is_empty() {
        // Edit in place; fresh post only when definitely gone
        // (edit_gone parity) — transient keeps the slot for the heal.
        let no_kb = Some(Value::Array(Vec::new()));
        match s
            .tg
            .try_edit_msg(chat, msg_id, crate::ui::UNKNOWN_BUTTON, no_kb)
            .await
        {
            Ok(()) => {
                s.remember(chat, Some(msg_id), pane).await;
            }
            Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                let mid =
                    s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_BUTTON, None)
                        .await;
                s.remember(chat, mid, pane).await;
                s.tg.strip_buttons(chat, msg_id).await;
            }
            Err(_) => {
                s.tg.strip_buttons(chat, msg_id).await;
            }
        }
    } else {
        let (q, opts) = live_card(&screen);
        let text = format!(
            "⚠️ that button expired — current question:\n\n{}",
            blocked_card_text(&q, &opts)
        );
        let kb = Some(blocked_kb(pane, &opts));
        match s.tg.try_edit_msg(chat, msg_id, &text, kb.clone()).await {
            Ok(()) => {
                s.blocked_sig
                    .lock()
                    .await
                    .insert(pane.to_string(), dialog_sig(&screen));
                s.remember(chat, Some(msg_id), pane).await;
                crate::handlers::dialog::settle_card(s, pane, chat, msg_id).await;
            }
            Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                if let Some(mid) = s.tg.send_msg(chat, thread, &text, kb).await {
                    s.blocked_sig
                        .lock()
                        .await
                        .insert(pane.to_string(), dialog_sig(&screen));
                    s.remember(chat, Some(mid), pane).await;
                    crate::handlers::dialog::settle_card(s, pane, chat, mid).await;
                    s.tg.strip_buttons(chat, msg_id).await;
                }
            }
            Err(_) => {
                s.tg.strip_buttons(chat, msg_id).await;
            }
        }
    }
    delayed_refresh(s, pane).await;
}
