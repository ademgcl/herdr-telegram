use super::tap_classify::dialog_moved;
use super::tap_keys::TapSend;
use super::tap_refresh::delayed_refresh;
use crate::{
    handlers::dialog::{blocked_card_text, blocked_kb, live_card, parse_options},
    state::AppState,
};

/// Unchanged-tap outcome: the keys landed but herdr still samples
/// `blocked`. Two sub-cases: the screen moved off the dialog (a resume
/// masked by status lag — follow speculatively for the final, the
/// still-blocked stand-down inside `follow_answer` keeps it exact), or
/// an unreadable re-read (follow too: a blip must not lose the turn).
/// A truly stuck dialog re-renders its buttons in place from the live
/// read (same content, no buzz — the sig already matches), so a
/// same-text re-prompt never strands buttonless.
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
    if dialog_moved(before, after) {
        let mut shown = send.nav.clone();
        shown.extend(send.confirm.iter().cloned());
        let sent = shown.join("+");
        // Fresh `after`, never stale `before`: a resumed turn (working
        // prose) must not claim the question is still up.
        let text = if parse_options(after).is_empty() {
            format!("⌨️ sent {sent} — check the pane")
        } else {
            format!(
                "⚠️ sent {sent} but the question is still up — /card for fresh buttons, /esc to dismiss, or answer on the PC"
            )
        };
        let mid = s.tg.send_silent(chat, thread, &text).await;
        s.remember(chat, mid, pane).await;
        s.tg.strip_buttons(chat, msg_id).await;
        s.blocked_sig.lock().await.remove(pane);
        drop(op);
        crate::jobs::follow::follow_answer(s, pane, chat, thread, &send.label).await;
        delayed_refresh(s, pane).await;
        return;
    }
    if after.is_empty() {
        // Unreadable re-read (blip): never touch the card — the sig stays
        // so the next refresh reposts nothing new, buttons stay armed, and
        // the follower (which polls status itself and stands down on
        // outage) still covers a resume hiding behind the blip.
        drop(op);
        crate::jobs::follow::follow_answer(s, pane, chat, thread, &send.label).await;
        delayed_refresh(s, pane).await;
        return;
    }
    // Stuck on the same dialog: converge the tapped card itself back to
    // live buttons from the FRESH read (before is seconds stale), edit in
    // place, never a fresh post — nothing new to buzz. A failed edit
    // falls back to one fresh card with buttons; anything else keeps
    // the slot for the heal below.
    let (q, opts) = live_card(after);
    let text = blocked_card_text(&q, &opts);
    let kb = Some(blocked_kb(pane, &opts));
    match s.tg.try_edit_msg(chat, msg_id, &text, kb.clone()).await {
        Ok(()) => {
            s.remember(chat, Some(msg_id), pane).await;
        }
        Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
            if let Some(mid) = s.tg.send_msg(chat, thread, &text, kb).await {
                s.remember(chat, Some(mid), pane).await;
                crate::handlers::dialog::settle_card(s, pane, chat, mid).await;
            }
            s.tg.strip_buttons(chat, msg_id).await;
        }
        Err(_) => {
            s.tg.strip_buttons(chat, msg_id).await;
        }
    }
    delayed_refresh(s, pane).await;
}
