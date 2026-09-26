/// `/kill`: close a pane completely (not quit-to-shell). Works from agent
/// or shell topics — mode doesn't matter. Irreversible, so it always
/// confirms first via stateless inline buttons (`X:kill:` / `X:keep:` carry
/// the pane; no pending-state map to leak).
use serde_json::{Value, json};

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

/// Kill/Keep keyboard (pure, tested): single source for the confirm
/// card AND the failure re-attach — an actionable edit must keep the
/// buttons (`edit_msg(None)` strips them, stranding keep-intent).
pub fn kill_kb(pane: &str) -> Value {
    json!([[
        {"text": "☠️ Kill", "callback_data": format!("X:kill:{pane}")},
        {"text": "Keep", "callback_data": format!("X:keep:{pane}")},
    ]])
}

/// Failure edit (pure, tested): confirmed-gone is terminal (no
/// buttons); a live pane keeps the Kill/Keep keyboard so the tap can
/// retry Kill or take Keep — never strip on the failure path.
pub fn kill_failure_edit(pane: &str, err: &str, gone: bool) -> (String, Option<Value>) {
    if gone {
        (format!("☠️ {pane} already closed."), None)
    } else {
        (
            format!("⚠️ kill failed: {}", crate::types::mask_home(err)),
            Some(kill_kb(pane)),
        )
    }
}

/// Describe the pane for the confirm card, or None when already gone.
/// Fail-closed: a failed pane list must not read as "gone" (every kill
/// would false-gone during a herdr blip); the kill itself then fails
/// gracefully with a visible error. Err on outage: posting the
/// destructive confirm card on an ambiguous read violates fail-closed.
async fn describe(s: &AppState, pane: &str) -> Result<Option<String>, ()> {
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) => return Ok(Some(format!("{}, {}", a.kind, a.status))),
        // Only confirmed death reads as shell (dm_info/tap_runkey
        // parity): a get_agent blip on a live agent refuses visibly
        // instead of mislabeling it "shell" on the confirm card.
        Err(e) if crate::herdr::rpc::should_retry_agent_lookup(&e.to_string()) => {
            return Err(());
        }
        Err(_) => {}
    }
    match list_panes(&s.cfg.socket).await {
        Ok(panes) if panes.contains(&pane.to_string()) => Ok(Some("shell".to_string())),
        Ok(_) => Ok(None),
        Err(_) => Err(()),
    }
}

/// Ask: post the confirm card with Kill/Keep buttons.
pub async fn ask_kill(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) {
    let desc = match describe(s, pane).await {
        Err(()) => {
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            return;
        }
        Ok(None) => {
            s.tg.send_msg(
                chat,
                thread,
                &format!("⚠️ pane {pane} is already gone"),
                None,
            )
            .await;
            return;
        }
        Ok(Some(desc)) => desc,
    };
    let mid =
        s.tg.send_msg(
            chat,
            thread,
            &kill_confirm_text(pane, &desc),
            Some(kill_kb(pane)),
        )
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
            let (text, kb) = kill_failure_edit(pane, &e.to_string(), gone);
            s.tg.edit_msg(chat, msg_id, &text, kb).await;
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

    #[test]
    fn test_kill_failure_keeps_keyboard_while_live() {
        // Keep-intent hole: the failure edit must re-attach Kill/Keep
        // (edit_msg(None) strips them — a failed kill then strands the
        // card with no retry and no Keep). Confirmed-gone is terminal.
        let (text, kb) = kill_failure_edit("w1:p1", "boom", false);
        assert!(text.contains("kill failed") && text.contains("boom"));
        let kb = kb.expect("live failure keeps the keyboard");
        assert_eq!(kb[0][0]["callback_data"], "X:kill:w1:p1");
        assert_eq!(kb[0][1]["callback_data"], "X:keep:w1:p1");
        let (text, kb) = kill_failure_edit("w1:p1", "x", true);
        assert!(text.contains("already closed"));
        assert!(kb.is_none(), "terminal ack never re-arms buttons");
        // Confirm card shares the same keyboard source.
        let kb = kill_kb("w1:p2");
        assert_eq!(kb[0][0]["callback_data"], "X:kill:w1:p2");
        assert_eq!(kb[0][1]["text"], "Keep");
    }
}
