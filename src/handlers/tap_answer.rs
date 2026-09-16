use super::tap_classify::{TapResult, classify_tap};
use super::tap_keys::{TapCall, tap_keys};
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, live_card, parse_options},
    herdr::client::read_screen_visible,
    state::AppState,
};
use serde_json::Value;
use std::time::Duration;

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
        // Exclusive waiter: drop sibling run/key waiters for this key so
        // the next message types instead of running.
        s.runwait.lock().await.remove(&(chat, thread));
        s.keywait.lock().await.remove(&(chat, thread));
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
    // Single-flight per pane: a double-tap (or two owners) must not
    // interleave key sequences into the same dialog, and observations
    // must not post over the card this tap owns. RAII: cancellation
    // mid-tap must not wedge the pane.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        let mid =
            s.tg.send_msg(chat, thread, "tap already in flight — wait a beat", None)
                .await;
        s.remember(chat, mid, pane).await;
        return;
    };
    println!("[tap] {pane} action={action}");
    let call = tap_keys(&s.cfg.socket, pane, action).await;
    match call {
        TapCall::Unknown => {
            // Narrowed/turned-over dialog: refresh the card in place with
            // the live option set instead of stranding dead buttons.
            let screen = read_screen_visible(&s.cfg.socket, pane, 30).await;
            if screen.is_empty() {
                let mid = s.tg.send_msg(chat, thread, "unknown button", None).await;
                s.remember(chat, mid, pane).await;
            } else {
                let (q, opts) = live_card(&screen);
                let text = format!(
                    "⚠️ that button expired — current dialog:\n\n{}",
                    blocked_card_text(&q)
                );
                let kb = Some(blocked_kb(pane, &opts));
                if s.tg
                    .try_edit_msg(chat, msg_id, &text, kb.clone())
                    .await
                    .is_ok()
                {
                    s.blocked_sig.lock().await.insert(pane.to_string(), q);
                    s.remember(chat, Some(msg_id), pane).await;
                } else if let Some(mid) = s.tg.send_msg(chat, thread, &text, kb).await {
                    s.blocked_sig.lock().await.insert(pane.to_string(), q);
                    s.remember(chat, Some(mid), pane).await;
                }
            }
            delayed_refresh(s, pane).await;
        }
        TapCall::KeysFailed => {
            let mid =
                s.tg.send_msg(chat, thread, "⚠️ keys failed — answer on the PC", None)
                    .await;
            s.remember(chat, mid, pane).await;
            delayed_refresh(s, pane).await;
        }
        TapCall::Landed(send, before, after, still_blocked) => {
            match classify_tap(&before, &after, still_blocked) {
                TapResult::NewDialog => {
                    let (q, opts) = live_card(&after);
                    let text = blocked_card_text(&q);
                    let kb = Some(blocked_kb(pane, &opts));
                    // Edit first; on failure post fresh — and stamp the
                    // signature ONLY on delivery, so a dropped update
                    // stays "new" for the watchdog instead of blinding it.
                    if s.tg
                        .try_edit_msg(chat, msg_id, &text, kb.clone())
                        .await
                        .is_ok()
                    {
                        let _ = s.tg.set_reaction(chat, msg_id, Some("❗")).await;
                        s.blocked_sig.lock().await.insert(pane.to_string(), q);
                        s.remember(chat, Some(msg_id), pane).await;
                    } else if let Some(mid) =
                        s.tg.send_msg_with_effect(
                            chat,
                            thread,
                            &text,
                            kb,
                            Some(crate::telegram::EFFECT_FIRE),
                        )
                        .await
                    {
                        let _ = s.tg.set_reaction(chat, mid, Some("❗")).await;
                        s.blocked_sig.lock().await.insert(pane.to_string(), q);
                        s.remember(chat, Some(mid), pane).await;
                    }
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
                    let _ = s.tg.set_reaction(chat, msg_id, Some("✅")).await;
                    // The status layer can lag up to a cycle behind: clear
                    // the dialog signature now or the next same-`blocked`
                    // observation reposts a ghost card for a live agent.
                    s.blocked_sig.lock().await.remove(pane);
                    s.remember(chat, Some(msg_id), pane).await;
                    delayed_refresh(s, pane).await;
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
                            "⚠️ sent {sent} but the dialog still shows — /card for fresh buttons, /esc to dismiss, or answer on the PC"
                        )
                    };
                    let mid = s.tg.send_msg(chat, thread, &text, None).await;
                    s.remember(chat, mid, pane).await;
                    delayed_refresh(s, pane).await;
                }
            }
        }
    }
}

/// Delayed self-heal for non-card outcomes (typed answers do the same):
/// slow renders and lagging status resolve in seconds via refresh
/// instead of the ≤60s watchdog.
async fn delayed_refresh(s: &AppState, pane: &str) {
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        crate::handlers::dialog::refresh_blocked_card(&s2, &pane2).await;
    });
}
