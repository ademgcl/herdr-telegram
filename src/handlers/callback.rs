use crate::{
    herdr::client::{
        get_agent, list_agents, list_panes, list_workspaces, read_agent_output, read_pane_output,
    },
    state::AppState,
    ui::{
        agent_card_kb, btn, build_agent_card_text, build_menu_text, build_ws_text, main_menu_kb,
        pane_output_kb, spawn_kb, workspace_kb,
    },
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// True when the pane still exists (agent or shell). Stale card taps
/// must never arm waiters, move focus, or remember targets for dead
/// panes — the next message would route into the void.
async fn pane_live(s: &AppState, pane: &str) -> bool {
    if get_agent(&s.cfg.socket, pane).await.is_ok() {
        return true;
    }
    // Fail-open: a failed list call must not read as "dead" (every stale
    // tap would false-gone during a herdr blip); the tap itself then
    // fails gracefully with a visible error.
    list_panes(&s.cfg.socket)
        .await
        .map(|l| l.contains(&pane.to_string()))
        .unwrap_or(true)
}

/// Dead pane tapped: retire the stale card, route nothing.
async fn gone_card(s: &AppState, chat: i64, msg_id: i64, pane: &str) {
    s.tg.edit_msg(chat, msg_id, &format!("pane {pane} is gone"), None)
        .await;
    s.targets.lock().await.remove(&(chat, msg_id));
}

/// Dead-pane guard for `B:`/`M:`/`X:` taps: parse the pane out of the
/// rest and retire the stale card when it is gone. Returns false when
/// the caller must stop.
async fn live_target(s: &AppState, chat: i64, msg_id: i64, r: &str) -> bool {
    match split_action(r) {
        Some((_, pane)) if !pane_live(s, pane).await => {
            gone_card(s, chat, msg_id, pane).await;
            false
        }
        _ => true,
    }
}

/// First routing cut: `B:opt2:wG:p1` → `("B", Some("opt2:wG:p1"))`.
/// Pure so the colon rules are unit-tested, not just eyeballed.
pub(crate) fn split_head(data: &str) -> (&str, Option<&str>) {
    match data.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (data, None),
    }
}

/// Second cut for pane-carrying actions: `opt2:wG:p1` →
/// `Some(("opt2", "wG:p1"))`. Pane ids contain ':' so only the FIRST
/// colon splits — multi-split thinking must never creep in.
pub(crate) fn split_action(rest: &str) -> Option<(&str, &str)> {
    rest.split_once(':')
}

pub async fn handle_callback(s: AppState, cbq: &Value) {
    let Some(from) = cbq["from"]["id"].as_i64() else {
        return;
    };
    if !s.cfg.owners.contains(&from) {
        println!("[callback] ignoring non-owner tap from {from}");
        return;
    }
    let chat = cbq["message"]["chat"]["id"].as_i64();
    let msg_id = cbq["message"]["message_id"].as_i64();
    let data = cbq["data"].as_str().unwrap_or("");

    if let Some(cbq_id) = cbq["id"].as_str() {
        s.tg.answer_callback(cbq_id).await;
    }
    let (Some(chat), Some(msg_id)) = (chat, msg_id) else {
        return;
    };

    // Thread for ack messages (forum topics carry it, DMs don't).
    let thread = cbq["message"]["message_thread_id"].as_i64();
    let date = cbq["message"]["date"].as_u64().unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_sub(date) > 86400 {
        println!("[callback] dropping stale tap");
        return;
    }
    let (head, rest) = split_head(data);
    match (head, rest) {
        ("n", None) => {
            s.tg.edit_msg(chat, msg_id, "spawn which agent?", Some(spawn_kb(None)))
                .await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("n", Some(ws)) => {
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("spawn into {ws}: which agent?"),
                Some(spawn_kb(Some(ws))),
            )
            .await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("N", None) => super::callback_spawn::handle_new_space(&s, chat, msg_id, thread).await,
        ("k", Some(r)) => match r.split_once(':') {
            Some((ws, kind)) => {
                super::callback_spawn::handle_spawn(&s, chat, msg_id, kind, Some(ws)).await
            }
            None => super::callback_spawn::handle_spawn(&s, chat, msg_id, r, None).await,
        },
        ("K", Some(pane)) => {
            if !pane_live(&s, pane).await {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            }
            s.keywait
                .lock()
                .await
                .insert((chat, thread), pane.to_string());
            s.set_focus(pane).await;
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"),
                None,
            )
            .await;
        }
        ("R", Some(ws)) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            if !spaces.iter().any(|w| w.id == *ws) {
                s.tg.edit_msg(chat, msg_id, &format!("workspace {ws} is gone"), None)
                    .await;
                s.targets.lock().await.remove(&(chat, msg_id));
                return;
            }
            s.runwait
                .lock()
                .await
                .insert((chat, thread), ws.to_string());
            s.tg.edit_msg(
                chat,
                msg_id,
                &format!("⌨️ send shell command for {ws}\nnext message = command"),
                None,
            )
            .await;
        }
        ("p", Some(pane)) => {
            let Ok(out) = read_pane_output(&s.cfg.socket, pane, 120).await else {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            };
            let body = if out.is_empty() {
                "(no output)".into()
            } else {
                out
            };
            let mid =
                s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(pane)))
                    .await;
            s.remember(chat, mid, pane).await;
        }
        ("m", None) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            s.tg.edit_msg(
                chat,
                msg_id,
                &build_menu_text(&spaces, &agents),
                Some(main_menu_kb(&spaces, &agents)),
            )
            .await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("w", Some(ws)) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            let my_agents: Vec<_> = agents.into_iter().filter(|a| a.ws == *ws).collect();
            s.tg.edit_msg(
                chat,
                msg_id,
                &build_ws_text(ws, &spaces, &my_agents),
                Some(workspace_kb(ws, &my_agents)),
            )
            .await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("a", Some(pane)) => {
            match get_agent(&s.cfg.socket, pane).await {
                Ok(agent) => {
                    s.remember(chat, Some(msg_id), pane).await;
                    s.set_focus(pane).await;
                    s.tg.edit_msg(
                        chat,
                        msg_id,
                        &build_agent_card_text(&agent),
                        Some(agent_card_kb(pane, &agent.ws)),
                    )
                    .await;
                }
                Err(_) => {
                    if pane_live(&s, pane).await {
                        // Agent gone but the shell lives: deliberate taps
                        // may still navigate here — focus moves, but there
                        // is no agent card to show.
                        s.remember(chat, Some(msg_id), pane).await;
                        s.set_focus(pane).await;
                        s.tg.edit_msg(
                            chat,
                            msg_id,
                            &format!("{pane} is now a shell pane (agent gone)"),
                            None,
                        )
                        .await;
                    } else {
                        gone_card(&s, chat, msg_id, pane).await;
                    }
                }
            }
        }
        ("o", Some(pane)) => {
            let Ok(out) = read_agent_output(&s.cfg.socket, pane, 120).await else {
                gone_card(&s, chat, msg_id, pane).await;
                return;
            };
            let body = if out.is_empty() {
                "(no output)".into()
            } else {
                out
            };
            let kb = serde_json::json!([[btn("← back", &format!("a:{pane}"))]]);
            let mid = s.tg.send_msg(chat, thread, &body, Some(kb)).await;
            s.remember(chat, mid, pane).await;
            s.set_focus(pane).await;
        }
        // Blocked-pane answers: B:<action>:<pane> — the tapped card is
        // updated in place (a turned-over dialog swaps question+buttons).
        ("B", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            if let Some((action, pane)) = split_action(r) {
                super::tap::answer_tap(&s, chat, msg_id, thread, pane, action).await;
            }
        }
        // Model picker: M:<idx>:<pane> free-Zen taps, M:list:<pane> card.
        ("M", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            super::callback_model::handle_model_tap(&s, chat, msg_id, thread, r).await
        }
        // Pane kill confirm: X:<kill|keep>:<pane> — stateless buttons.
        ("X", Some(r)) => {
            if !live_target(&s, chat, msg_id, r).await {
                return;
            }
            if let Some((action, pane)) = split_action(r) {
                super::kill::handle_kill_action(&s, chat, msg_id, action, pane).await;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_head() {
        assert_eq!(split_head("n"), ("n", None));
        assert_eq!(split_head("B:opt2:wG:p1"), ("B", Some("opt2:wG:p1")));
        assert_eq!(split_head("X:kill:w1:p2"), ("X", Some("kill:w1:p2")));
    }

    #[test]
    fn test_split_action_keeps_pane_whole() {
        // Pane ids contain ':' — only the first colon splits.
        assert_eq!(split_action("opt2:wG:p1"), Some(("opt2", "wG:p1")));
        assert_eq!(split_action("kill:w1:p2"), Some(("kill", "w1:p2")));
        assert_eq!(split_action("allow"), None);
    }
}
