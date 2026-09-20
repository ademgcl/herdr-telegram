use crate::{herdr::client::get_agent, state::AppState};
use serde_json::Value;

/// Single source for the stale-picker repost (both arms rebuild the
/// same fresh picker — dup'd literals re-drift).
pub(crate) fn stale_picker_text(cur: Option<&str>, pane: &str, kind: &str) -> String {
    format!(
        "⚠️ that button was stale — fresh list, tap again:\n\n{}",
        super::model::model_card_text(cur, pane, kind)
    )
}

/// Delivery-gated pin verdict (pure, tested): a deleted card
/// (`edit_gone`) must not pin DM focus/routing, and a read-only forum
/// view must not reroute global DM traffic (DM-only focus). Single
/// source for the `M:list` arm below.
pub(crate) fn model_list_pins_focus(thread: Option<i64>, edit_ok: bool) -> bool {
    edit_ok && thread.is_none()
}

/// Model-card taps: `M:list:<pane>` re-renders the card in place,
/// `M:<idx>:<pane>` switches to that free-Zen model with progress edits.
pub(crate) async fn handle_model_tap(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    thread: Option<i64>,
    r: &str,
) {
    let Some((idx, pane)) = r.split_once(':') else {
        // Malformed tap (no pane): visible ack like B:/X: arms, never a
        // silent drop — a dead spinner looks wedged.
        s.tg.edit_msg(chat, msg_id, crate::ui::UNKNOWN_BUTTON, None)
            .await;
        return;
    };
    if idx == "list" {
        // Fail-closed like the K/R arms: an unreadable herdr never moves
        // focus, remembers routing, or renders a "?" card (ambiguous
        // read → no write, visible retry).
        let Ok(agent) = get_agent(&s.cfg.socket, pane).await else {
            s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return;
        };
        let kind = agent.kind;
        let cur = super::model::current_model(s, pane).await;
        let text = super::model::model_card_text(cur.as_deref(), pane, &kind);
        let kb = if kind == "opencode" {
            Some(super::model::model_kb(pane))
        } else {
            None
        };
        // Routing follows delivery (show_model parity): a deleted card
        // (edit_gone) must not pin DM focus/routing, and a read-only
        // forum view must not reroute global DM traffic (DM-only focus).
        let edit_ok = s.tg.try_edit_msg(chat, msg_id, &text, kb).await.is_ok();
        if edit_ok {
            s.remember(chat, Some(msg_id), pane).await;
        }
        if model_list_pins_focus(thread, edit_ok) {
            s.set_focus(pane).await;
        }
        return;
    }
    let Ok(i) = idx.parse::<usize>() else {
        s.tg.edit_msg(chat, msg_id, crate::ui::UNKNOWN_BUTTON, None)
            .await;
        return;
    };
    let Some((filter, marker)) = super::model_parse::free_tap(i) else {
        // Out-of-range index (stale shortlist button): fresh picker beats
        // a dead end — the next tap can't miss. Fail-closed like M:list:
        // an unreadable herdr refuses instead of rendering a "?" card.
        let Ok(agent) = get_agent(&s.cfg.socket, pane).await else {
            s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return;
        };
        let kind = agent.kind;
        let cur = super::model::current_model(s, pane).await;
        let text = stale_picker_text(cur.as_deref(), pane, &kind);
        let kb = if kind == "opencode" {
            Some(super::model::model_kb(pane))
        } else {
            None
        };
        s.tg.edit_msg(chat, msg_id, &text, kb).await;
        return;
    };
    // Single-flight BEFORE the progress edit (tap_answer parity): two
    // queued taps must not both paint progress with last-writer-wins on
    // the terminal card.
    if s.model_held(pane).await {
        s.tg.send_msg(chat, thread, crate::ui::TAP_MODEL_IN_FLIGHT, None)
            .await;
        return;
    }
    // Reset/remint guard (`/model <filter>` parity): a reset landing
    // between card render and tap must not drive picker keys into the
    // new generation's live work. DM has no thread mapping to check.
    let gen_ok = match thread {
        None => !crate::handlers::reset::is_resetting(),
        Some(t) => super::forum_topic_status::model_switch_allowed(
            crate::handlers::reset::is_resetting(),
            s.topics.all_mappings().get(pane).copied(),
            t,
        ),
    };
    if !gen_ok {
        // Visible, never silent: the spinner already stopped and the
        // card keeps live buttons — without feedback the tap reads as
        // wedged. A message (never a card edit) so the picker survives
        // for a retry after the reset / in the live topic.
        let notice = if crate::handlers::reset::is_resetting() {
            crate::ui::RESET_BUSY
        } else {
            crate::ui::UNKNOWN_BUTTON
        };
        s.tg.send_msg(chat, thread, notice, None).await;
        return;
    }
    // Strip the buttons while switching: mid-switch taps can only collide.
    let no_kb = Some(Value::Array(Vec::new()));
    s.tg.edit_msg(
        chat,
        msg_id,
        &super::model::switch_progress(pane, &marker),
        no_kb.clone(),
    )
    .await;
    match super::model::switch_model(s, pane, &filter, &marker).await {
        // Set means set: plain confirmation, buttons stay off. Focus
        // follows success only — a failed switch must not stick focus
        // to a pane that can't switch.
        Ok(footer) => {
            // Routing follows delivery (M:list parity above): a deleted
            // card (edit_gone) must not pin DM focus/routing to a dead
            // msg_id.
            if s.tg
                .try_edit_msg(
                    chat,
                    msg_id,
                    &super::model::switch_done(pane, &footer),
                    Some(Value::Array(Vec::new())),
                )
                .await
                .is_ok()
            {
                // DM-only focus (M:list parity above): a forum switch is
                // thread-routed and must not reroute global DM traffic.
                if thread.is_none() {
                    s.set_focus(pane).await;
                }
                s.remember(chat, Some(msg_id), pane).await;
            }
        }
        Err(e) => {
            if e.to_string().starts_with("no model matches") {
                // Button predates the picker-grounded rename (e.g. the old
                // "Contributor" filter): swap the dead card for a fresh one
                // so the next tap can't miss. Fail-closed like M:list.
                let Ok(agent) = get_agent(&s.cfg.socket, pane).await else {
                    s.tg.edit_msg(chat, msg_id, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                };
                let kind = agent.kind;
                let cur = super::model::current_model(s, pane).await;
                let text = stale_picker_text(cur.as_deref(), pane, &kind);
                let kb = if kind == "opencode" {
                    Some(super::model::model_kb(pane))
                } else {
                    None
                };
                s.tg.edit_msg(chat, msg_id, &text, kb).await;
            } else {
                // Keep the picker on screen so a retry is one tap.
                // Fail-closed like the success arm: a deleted card pins
                // no routing (no remember — the next tap re-renders).
                let cur = super::model::current_model(s, pane).await;
                let cur_line = cur.as_deref().unwrap_or("(unreadable)");
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &format!(
                        "⚠️ switch failed: {}\nstill on: {cur_line}",
                        crate::types::mask_home(&e.to_string())
                    ),
                    Some(super::model::model_kb(pane)),
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_list_pins_focus_dm_only_on_delivery() {
        // DM tap with a delivered edit pins focus.
        assert!(model_list_pins_focus(None, true));
        // Deleted card (edit_gone) pins nothing — no focus, and the
        // caller skips `remember` on the same verdict.
        assert!(!model_list_pins_focus(None, false));
        // Forum view never reroutes global DM traffic, even delivered.
        assert!(!model_list_pins_focus(Some(7), true));
        assert!(!model_list_pins_focus(Some(7), false));
    }
}
