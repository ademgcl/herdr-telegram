use super::tap_classify::{TapResult, classify_tap};
use super::tap_keys::{TapCall, tap_keys};
use super::tap_refresh::delayed_refresh;
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, dialog_sig, live_card},
    herdr::client::{get_agent, read_screen_visible},
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
        super::tap_input::arm_type_waiter(s, chat, thread, pane).await;
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
        if tw
            .get(&(chat, thread))
            .map(|(p, _)| p == pane)
            .unwrap_or(false)
        {
            tw.remove(&(chat, thread));
        }
    }
    // Stale-card identity gate: turnover repoints tracking (old card
    // stripped, new tracked), so a queued tap from a superseded card
    // must not drive keys into the new dialog's different option set
    // (bounds alone can't tell same-length turnovers apart). Tracked
    // but untracked-tap → skip sends, fall into the Unknown refresh
    // below. Empty map = restart-emptied, stay permissive.
    let stale_card = {
        let map = s.blocked_card.lock().await;
        match map.get(pane) {
            Some(v) if !v.is_empty() => !v.contains(&(chat, msg_id)),
            _ => false,
        }
    };
    // Optimistic claim, BEFORE the slow herdr roundtrips: strip the
    // tapped card's buttons so the tap reads instant and cannot
    // double-fire. Text is untouched (markup-only), so every outcome
    // below only ever overwrites — the card converges by construction
    // and can never strand live buttons on a dead end.
    s.tg.strip_buttons(chat, msg_id).await;
    println!("[tap] {pane} action={action}");
    let call = if stale_card {
        TapCall::Unknown
    } else {
        tap_keys(&s.cfg.socket, pane, action).await
    };
    match call {
        TapCall::Unknown => {
            // Narrowed/turned-over dialog: refresh the card in place with
            // the live option set instead of stranding dead buttons. But
            // a stale tap on a LIVE pane must strip, never ghost: posting
            // blocked buttons for working output invites blind taps (the
            // gate already refuses them, but the card must not offer).
            // Fail-closed: an unreadable status posts no buttons and no
            // buzz — an outage must never mint live buttons from a guess.
            let live_blocked = match get_agent(&s.cfg.socket, pane).await {
                Ok(a) => a.status == "blocked",
                Err(_) => {
                    s.tg.strip_buttons(chat, msg_id).await;
                    s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
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
                // Sibling surfaces (DM owners) may still show this dead
                // dialog with live buttons — the heal below resolves them.
                delayed_refresh(s, pane).await;
                return;
            }
            let screen = read_screen_visible(&s.cfg.socket, pane, 30).await;
            if screen.is_empty() {
                // Edit in place (no fresh-message accumulation): every
                // other tap arm converges the tapped card via edit first.
                // Fresh post only when the card is definitely gone
                // (edit_gone parity with Resumed/NewDialog) — transient
                // keeps the slot for the heal below.
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
                        // Unverifiable tap keeps no buttons: heal re-renders below.
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
                        // This card is current — strip sibling surfaces.
                        crate::handlers::dialog::settle_card(s, pane, chat, msg_id).await;
                    }
                    Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                        if let Some(mid) = s.tg.send_msg(chat, thread, &text, kb).await {
                            s.blocked_sig
                                .lock()
                                .await
                                .insert(pane.to_string(), dialog_sig(&screen));
                            s.remember(chat, Some(mid), pane).await;
                            // Settle the fresh surface; the tapped card may be
                            // untracked (pre-restart post) — strip it too.
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
        TapCall::KeysFailed => {
            let mid =
                s.tg.send_msg(chat, thread, crate::ui::KEYS_FAILED_PC, None)
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
                    let text = blocked_card_text(&q, &opts);
                    let kb = Some(blocked_kb(pane, &opts));
                    // Edit first; a fresh post only when the card is
                    // definitely gone (edit_gone parity with report.rs) —
                    // a transient failure keeps the slot and heals below
                    // instead of duplicating beside the stripped corpse.
                    // Signature stamps ONLY on delivery, so a dropped
                    // update stays "new" for the watchdog.
                    match s.tg.try_edit_msg(chat, msg_id, &text, kb.clone()).await {
                        Ok(()) => {
                            let _ = s.tg.set_reaction(chat, msg_id, Some("❗")).await;
                            s.blocked_sig
                                .lock()
                                .await
                                .insert(pane.to_string(), dialog_sig(&after));
                            s.remember(chat, Some(msg_id), pane).await;
                            // This card is current — strip sibling surfaces.
                            crate::handlers::dialog::settle_card(s, pane, chat, msg_id).await;
                        }
                        Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                            if let Some(mid) =
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
                                s.blocked_sig
                                    .lock()
                                    .await
                                    .insert(pane.to_string(), dialog_sig(&after));
                                s.remember(chat, Some(mid), pane).await;
                                // Settle the fresh surface; the tapped card
                                // may be untracked (pre-restart post) —
                                // strip it too.
                                crate::handlers::dialog::settle_card(s, pane, chat, mid).await;
                                s.tg.strip_buttons(chat, msg_id).await;
                            }
                        }
                        Err(_) => {
                            s.tg.strip_buttons(chat, msg_id).await;
                            delayed_refresh(s, pane).await;
                        }
                    }
                }
                TapResult::Resumed => {
                    let no_kb = Some(Value::Array(Vec::new()));
                    let done = format!("✅ {} answered — agent resumed [{pane}]", send.label);
                    // Same converging rule: a failed edit must not strand
                    // live buttons, but only a gone card earns a fresh
                    // post — transient keeps the slot for the heal below
                    // (sig is cleared, so nothing else repairs this card:
                    // strip it buttonless now).
                    match s.tg.try_edit_msg(chat, msg_id, &done, no_kb).await {
                        Ok(()) => {
                            let _ = s.tg.set_reaction(chat, msg_id, Some("✅")).await;
                            s.remember(chat, Some(msg_id), pane).await;
                        }
                        Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                            let mid = s.tg.send_msg(chat, thread, &done, None).await;
                            s.remember(chat, mid, pane).await;
                            s.tg.strip_buttons(chat, msg_id).await;
                        }
                        Err(_) => {
                            s.tg.strip_buttons(chat, msg_id).await;
                        }
                    }
                    // The status layer can lag up to a cycle behind: clear
                    // the dialog signature now or the next same-`blocked`
                    // observation reposts a ghost card for a live agent.
                    // (Routing memory is set per-branch above: the live
                    // card on edit, the fresh message on fallback.)
                    s.blocked_sig.lock().await.remove(pane);
                    // Follow resumed work to its final reply (spontaneous loses to a racing prompt).
                    drop(_op);
                    crate::jobs::follow::follow_answer(s, pane, chat, thread, &send.label).await;
                    delayed_refresh(s, pane).await;
                }
                TapResult::Unchanged => {
                    super::tap_unchanged::handle_unchanged(
                        s, chat, msg_id, thread, pane, &send, &before, &after, _op,
                    )
                    .await;
                }
            }
        }
    }
}
