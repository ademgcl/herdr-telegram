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
    format!(
        "☠️ kill {pane} ({desc})?\nThis closes the pane completely — agent or shell, work and all."
    )
}

/// Describe the pane for the confirm card, or None when already gone.
/// Fail-open: a failed pane list must not read as "gone" (every kill
/// would false-gone during a herdr blip); the kill itself then fails
/// gracefully with a visible error.
async fn describe(s: &AppState, pane: &str) -> Option<String> {
    if let Ok(a) = get_agent(&s.cfg.socket, pane).await {
        return Some(format!("{}, {}", a.kind, a.status));
    }
    match list_panes(&s.cfg.socket).await {
        Ok(panes) if panes.contains(&pane.to_string()) => Some("shell".to_string()),
        Ok(_) => None,
        Err(_) => Some("unreachable — assuming live".to_string()),
    }
}

/// Ask: post the confirm card with Kill/Keep buttons.
pub async fn ask_kill(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let Some(desc) = describe(s, pane).await else {
        s.tg.send_msg(
            chat,
            thread,
            &format!("⚠️ pane {pane} is already gone"),
            None,
        )
        .await;
        return;
    };
    let kb = json!([[
        {"text": "☠️ Kill", "callback_data": format!("X:kill:{pane}")},
        {"text": "Keep", "callback_data": format!("X:keep:{pane}")},
    ]]);
    let mid =
        s.tg.send_msg(chat, thread, &kill_confirm_text(pane, &desc), Some(kb))
            .await;
    s.remember(chat, mid, pane).await;
}

/// Tap handler for the confirm buttons. Edits in place; killing also
/// retires jobs and drops the topic mapping immediately (the watchdog
/// would close it next cycle anyway).
pub async fn handle_kill_action(s: &AppState, chat: i64, msg_id: i64, action: &str, pane: &str) {
    if action == "keep" {
        s.tg.edit_msg(chat, msg_id, &format!("kept {pane}."), None)
            .await;
        return;
    }
    if action != "kill" {
        return;
    }
    // Snapshot the topic thread BEFORE any RPC: a remint racing the
    // close keeps its fresh topic (snapshot-id RPC + compare-delete
    // below never touch it).
    let thread = s.topics.all_mappings().get(pane).copied();
    match close_pane(&s.cfg.socket, pane).await {
        Ok(()) => {
            // Quiet: the "☠️ killed" edit below is the ack — a loud
            // cancel would add a stray "✋ cancelled" card next to it.
            // Generation gate: a remint racing the close RPC owns the
            // slot now — bail before touching fresh state (the watchdog
            // reconciles the closed pane next tick).
            let cur = s.topics.all_mappings().get(pane).copied();
            if cur != thread && cur.is_some() {
                s.tg.edit_msg(chat, msg_id, &format!("☠️ killed {pane}."), None)
                    .await;
                return;
            }
            s.cancel_jobs_for_quiet(pane).await;
            // Second generation gate (mirrors reconcile_close): a remint
            // landing in the cancel awaits above keeps its fresh state —
            // clear only the corpse's (live foreign mapping bails; the
            // watchdog reconciles the closed pane next tick).
            let cur2 = s.topics.all_mappings().get(pane).copied();
            if cur2 != thread && cur2.is_some() {
                s.tg.edit_msg(chat, msg_id, &format!("☠️ killed {pane}."), None)
                    .await;
                return;
            }
            s.clear_pane(pane).await;
            // Race-free: the snapshot id rides the RPC directly (no
            // re-read); the mapping compare-deletes only when still
            // current. Snapshot None means no known thread: skip — any
            // mapping present now is a remint whose topic must survive.
            if let Some(t) = thread {
                s.topics.close_topic_for_thread(pane, t).await;
            }
            s.tg.edit_msg(chat, msg_id, &format!("☠️ killed {pane}."), None)
                .await;
        }
        Err(e) => {
            // Double-tap lands here (first tap closed it): confirm gone
            // via the pane list so the ack reads "already closed", not a
            // failure. List errors stay a failure (fail-open would lie).
            let gone = list_panes(&s.cfg.socket)
                .await
                .map(|l| !l.contains(&pane.to_string()))
                .unwrap_or(false);
            if gone {
                s.tg.edit_msg(chat, msg_id, &format!("☠️ {pane} already closed."), None)
                    .await;
            } else {
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &format!(
                        "⚠️ kill failed: {}",
                        crate::types::mask_home(&e.to_string())
                    ),
                    None,
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
    fn test_kill_confirm_text() {
        let t = kill_confirm_text("w1:p1", "opencode, idle");
        assert!(t.contains("w1:p1"));
        assert!(t.contains("opencode, idle"));
        assert!(t.contains("completely"));
    }
}
