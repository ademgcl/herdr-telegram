/// `/kill`: close a pane completely (not quit-to-shell). Works from agent
/// or shell topics — mode doesn't matter. Irreversible, so it always
/// confirms first via stateless inline buttons (`X:kill:` / `X:keep:` carry
/// the pane; no pending-state map to leak).
use serde_json::json;

use crate::{
    herdr::client::{close_pane, get_agent, list_panes},
    state::AppState,
};

/// Pure confirm copy so tests cover it without I/O.
pub fn kill_confirm_text(pane: &str, desc: &str) -> String {
    format!("☠️ kill {pane} ({desc})?\nThis closes the pane completely — agent or shell, work and all.")
}

/// Describe the pane for the confirm card, or None when already gone.
async fn describe(s: &AppState, pane: &str) -> Option<String> {
    if let Ok(a) = get_agent(&s.cfg.socket, pane).await {
        return Some(format!("{}, {}", a.kind, a.status));
    }
    if list_panes(&s.cfg.socket).await.unwrap_or_default().contains(&pane.to_string()) {
        return Some("shell".to_string());
    }
    None
}

/// Ask: post the confirm card with Kill/Keep buttons.
pub async fn ask_kill(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let Some(desc) = describe(s, pane).await else {
        s.tg.send_msg(chat, thread, &format!("⚠️ pane {pane} is already gone"), None).await;
        return;
    };
    let kb = json!([[
        {"text": "☠️ Kill", "callback_data": format!("X:kill:{pane}")},
        {"text": "Keep", "callback_data": format!("X:keep:{pane}")},
    ]]);
    let mid = s.tg.send_msg(chat, thread, &kill_confirm_text(pane, &desc), Some(kb)).await;
    s.remember(chat, mid, pane).await;
}

/// Tap handler for the confirm buttons. Edits in place; killing also
/// retires jobs and drops the topic mapping immediately (the watchdog
/// would close it next cycle anyway).
pub async fn handle_kill_action(s: &AppState, chat: i64, msg_id: i64, action: &str, pane: &str) {
    if action == "keep" {
        s.tg.edit_msg(chat, msg_id, &format!("kept {pane}."), None).await;
        return;
    }
    if action != "kill" {
        return;
    }
    match close_pane(&s.cfg.socket, pane).await {
        Ok(()) => {
            s.cancel_jobs_for(pane).await;
            s.clear_pane(pane).await;
            s.topics.close_topic(pane).await;
            s.topics.remove_mapping(pane);
            s.tg.edit_msg(chat, msg_id, &format!("☠️ killed {pane}."), None).await;
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ kill failed: {e}"), None).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kill_confirm_text() {
        let t = kill_confirm_text("w1:p1", "opencode, idle");
        assert!(t.contains("w1:p1"));
        assert!(t.contains("opencode, idle"));
        assert!(t.contains("completely"));
    }
}
