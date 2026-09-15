use super::tap_classify::{TapResult, classify_tap};
use super::tap_keys::{TapCall, tap_keys};
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, parse_options, waiting_lines},
    state::AppState,
};
use serde_json::Value;

/// Button-tap entry point: sends keys, then brings the TAPPED card up
/// to date in place — a turned-over dialog swaps question + buttons, a
/// resumed agent gets its buttons stripped (stale taps must never inject
/// keys into live work). Feedback rides the card edit; only ambiguous
/// outcomes send a message.
pub async fn answer_tap(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    pane: &str,
    action: &str,
) {
    if action == "type" {
        s.typewait
            .lock()
            .await
            .insert((chat, thread), pane.to_string());
        let mid =
            s.tg.send_msg(
                chat,
                thread,
                "⌨️ type your answer as the next message (⏎ sends it)",
                None,
            )
            .await;
        s.remember(chat, mid, pane).await;
        return;
    }
    s.blockop.lock().await.insert(pane.to_string());
    let call = tap_keys(&s.cfg.socket, pane, action).await;
    s.blockop.lock().await.remove(pane);
    match call {
        TapCall::Unknown => {
            let mid = s.tg.send_msg(chat, thread, "unknown button", None).await;
            s.remember(chat, mid, pane).await;
        }
        TapCall::KeysFailed => {
            let mid =
                s.tg.send_msg(chat, thread, "⚠️ keys failed — answer on the PC", None)
                    .await;
            s.remember(chat, mid, pane).await;
        }
        TapCall::Landed(send, before, after, still_blocked) => {
            match classify_tap(&before, &after, still_blocked) {
                TapResult::NewDialog => {
                    let q = waiting_lines(&after);
                    let opts = parse_options(&after);
                    s.tg.edit_msg(
                        chat,
                        msg_id,
                        &blocked_card_text(&q),
                        Some(blocked_kb(pane, &opts)),
                    )
                    .await;
                    s.blocked_sig.lock().await.insert(pane.to_string(), q);
                    s.remember(chat, Some(msg_id), pane).await;
                }
                TapResult::Resumed => {
                    let no_kb = Some(Value::Array(Vec::new()));
                    s.tg.edit_msg(
                        chat,
                        msg_id,
                        &format!("✅ {} answered — agent resumed [{pane}]", send.label),
                        no_kb,
                    )
                    .await;
                    s.remember(chat, Some(msg_id), pane).await;
                }
                TapResult::Unchanged => {
                    let mut shown = send.nav.clone();
                    shown.extend(send.confirm.iter().cloned());
                    let sent = shown.join("+");
                    let before_opts = parse_options(&before);
                    let text = if before_opts.is_empty() {
                        format!("⌨️ sent {sent} — check the pane")
                    } else {
                        format!(
                            "⚠️ sent {sent} but the dialog still shows — highlight may have moved; try Esc or answer on the PC"
                        )
                    };
                    let mid = s.tg.send_msg(chat, thread, &text, None).await;
                    s.remember(chat, mid, pane).await;
                }
            }
        }
    }
}
