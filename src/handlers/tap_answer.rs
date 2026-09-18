use super::tap_classify::{TapResult, classify_tap};
use super::tap_keys::{TapCall, tap_keys};
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, live_card, parse_options},
    herdr::client::{get_agent, read_screen_visible},
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
    // mid-tap must not wedge the pane. Contention drops PURELY silent:
    // the spinner already stopped and the winner owns this pane — even
    // stripping our own card is unsafe here, a retried strip landing
    // after the winner's final edit would wipe its fresh buttons with
    // no heal repairing them. A stranded orphan self-heals: the next
    // tap re-validates through the blocked-gate + Unknown arm.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        return;
    };
    // A button tap supersedes an armed typed answer for THIS pane: drop
    // the waiter so the next message prompts instead of typing into the
    // turned-over dialog the tap just answered. After the claim (a
    // contended tap must not eat the waiter) and only on pane match —
    // DM shares one (chat,None) key across panes.
    {
        let mut tw = s.typewait.lock().await;
        if tw.get(&(chat, thread)).map(|p| p == pane).unwrap_or(false) {
            tw.remove(&(chat, thread));
        }
    }
    // Optimistic claim, BEFORE the slow herdr roundtrips: strip the
    // tapped card's buttons so the tap reads instant and cannot
    // double-fire. Text is untouched (markup-only), so every outcome
    // below only ever overwrites — the card converges by construction
    // and can never strand live buttons on a dead end.
    s.tg.strip_buttons(chat, msg_id).await;
    println!("[tap] {pane} action={action}");
    let call = tap_keys(&s.cfg.socket, pane, action).await;
    match call {
        TapCall::Unknown => {
            // Narrowed/turned-over dialog: refresh the card in place with
            // the live option set instead of stranding dead buttons. But
            // a stale tap on a LIVE pane must strip, never ghost: posting
            // blocked buttons for working output invites blind taps (the
            // gate already refuses them, but the card must not offer).
            let live_blocked = get_agent(&s.cfg.socket, pane)
                .await
                .map(|a| a.status == "blocked")
                .unwrap_or(true);
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
                return;
            }
            let screen = read_screen_visible(&s.cfg.socket, pane, 30).await;
            if screen.is_empty() {
                let mid = s.tg.send_msg(chat, thread, "unknown button", None).await;
                s.remember(chat, mid, pane).await;
                // Unverifiable tap keeps no buttons: heal re-renders below.
                s.tg.strip_buttons(chat, msg_id).await;
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
                    // Same orphan rule as the NewDialog fallback below.
                    s.tg.strip_buttons(chat, msg_id).await;
                }
            }
            delayed_refresh(s, pane).await;
        }
        TapCall::KeysFailed => {
            let mid =
                s.tg.send_msg(chat, thread, "⚠️ keys failed — answer on the PC", None)
                    .await;
            s.remember(chat, mid, pane).await;
            // Converge the tapped card (pending strip may have failed):
            // original text stands, buttons come off, heal follows.
            s.tg.strip_buttons(chat, msg_id).await;
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
                        // The old card must not keep offering superseded
                        // buttons next to the fresh one — strip it.
                        s.tg.strip_buttons(chat, msg_id).await;
                    }
                }
                TapResult::Resumed => {
                    let no_kb = Some(Value::Array(Vec::new()));
                    let done =
                        format!("✅ {} answered — agent resumed [{pane}]", send.label);
                    // Fallible edit with a converging fallback: a failed
                    // edit must not strand live buttons (sig is cleared
                    // below, so nothing else repairs this card).
                    if s.tg
                        .try_edit_msg(chat, msg_id, &done, no_kb)
                        .await
                        .is_ok()
                    {
                        let _ = s.tg.set_reaction(chat, msg_id, Some("✅")).await;
                        s.remember(chat, Some(msg_id), pane).await;
                    } else {
                        let mid = s.tg.send_msg(chat, thread, &done, None).await;
                        s.remember(chat, mid, pane).await;
                        s.tg.strip_buttons(chat, msg_id).await;
                    }
                    // The status layer can lag up to a cycle behind: clear
                    // the dialog signature now or the next same-`blocked`
                    // observation reposts a ghost card for a live agent.
                    // (Routing memory is set per-branch above: the live
                    // card on edit, the fresh message on fallback.)
                    s.blocked_sig.lock().await.remove(pane);
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
                    // Dead-end card keeps no live buttons: the explainer
                    // above carries the way out, heal re-renders below.
                    s.tg.strip_buttons(chat, msg_id).await;
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
