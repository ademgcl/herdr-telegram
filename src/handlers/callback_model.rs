use crate::{herdr::client::get_agent, state::AppState};
use serde_json::Value;

/// Model-card taps: `M:list:<pane>` re-renders the card in place,
/// `M:<idx>:<pane>` switches to that free-Zen model with progress edits.
pub(crate) async fn handle_model_tap(
    s: &AppState,
    chat: i64,
    msg_id: i64,
    _thread: Option<i64>,
    r: &str,
) {
    let Some((idx, pane)) = r.split_once(':') else {
        return;
    };
    if idx == "list" {
        let kind = get_agent(&s.cfg.socket, pane)
            .await
            .map(|a| a.kind)
            .unwrap_or_else(|_| "?".into());
        let cur = super::model::current_model(s, pane).await;
        let text = super::model::model_card_text(cur.as_deref(), pane, &kind);
        let kb = if kind == "opencode" {
            Some(super::model::model_kb(pane))
        } else {
            None
        };
        s.remember(chat, Some(msg_id), pane).await;
        s.set_focus(pane).await;
        s.tg.edit_msg(chat, msg_id, &text, kb).await;
        return;
    }
    let Ok(i) = idx.parse::<usize>() else { return };
    let Some((filter, marker)) = super::model_parse::free_tap(i) else {
        return;
    };
    s.set_focus(pane).await;
    // Strip the buttons while switching: mid-switch taps can only collide.
    let no_kb = Some(Value::Array(Vec::new()));
    s.tg.edit_msg(
        chat,
        msg_id,
        &format!("⏳ switching {pane} → `{marker}`…"),
        no_kb.clone(),
    )
    .await;
    match super::model::switch_model(s, pane, &filter, &marker).await {
        // Set means set: plain confirmation, buttons stay off.
        Ok(footer) => {
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("✅ model set: `{footer}`\n[{pane}]"),
                Some(Value::Array(Vec::new())),
            )
            .await;
        }
        Err(e) => {
            if e.starts_with("no model matches") {
                // Button predates the picker-grounded rename (e.g. the old
                // "Contributor" filter): swap the dead card for a fresh one
                // so the next tap can't miss.
                let kind = get_agent(&s.cfg.socket, pane)
                    .await
                    .map(|a| a.kind)
                    .unwrap_or_else(|_| "?".into());
                let cur = super::model::current_model(s, pane).await;
                let text = format!(
                    "⚠️ that button was stale — fresh list, tap again:\n\n{}",
                    super::model::model_card_text(cur.as_deref(), pane, &kind)
                );
                let kb = if kind == "opencode" {
                    Some(super::model::model_kb(pane))
                } else {
                    None
                };
                s.tg.edit_msg(chat, msg_id, &text, kb).await;
            } else {
                // Keep the picker on screen so a retry is one tap.
                let cur = super::model::current_model(s, pane).await;
                let cur_line = cur.as_deref().unwrap_or("(unreadable)");
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &format!("⚠️ switch failed: {e}\nstill on: {cur_line}"),
                    Some(super::model::model_kb(pane)),
                )
                .await;
            }
        }
    }
    // Card edits happen in place (same thread), so only routing memory
    // needs updating here.
    s.remember(chat, Some(msg_id), pane).await;
}
