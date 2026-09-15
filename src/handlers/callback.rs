use crate::{
    herdr::client::{get_agent, list_agents, list_workspaces, read_agent_output, read_pane_output},
    state::AppState,
    ui::{
        agent_card_kb, btn, build_agent_card_text, build_menu_text, build_ws_text, main_menu_kb,
        pane_output_kb, spawn_kb, workspace_kb,
    },
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let (head, rest) = match data.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (data, None),
    };
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
            let out = read_pane_output(&s.cfg.socket, pane, 120)
                .await
                .unwrap_or_default();
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
            s.remember(chat, Some(msg_id), pane).await;
            s.set_focus(pane).await;
            if let Ok(agent) = get_agent(&s.cfg.socket, pane).await {
                s.tg.edit_msg(
                    chat,
                    msg_id,
                    &build_agent_card_text(&agent),
                    Some(agent_card_kb(pane, &agent.ws)),
                )
                .await;
            }
        }
        ("o", Some(pane)) => {
            let out = read_agent_output(&s.cfg.socket, pane, 120)
                .await
                .unwrap_or_default();
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
            if let Some((action, pane)) = r.split_once(':') {
                super::tap::answer_tap(&s, chat, msg_id, thread, pane, action).await;
            }
        }
        // Model picker: M:<idx>:<pane> free-Zen taps, M:list:<pane> card.
        ("M", Some(r)) => {
            super::callback_model::handle_model_tap(&s, chat, msg_id, thread, r).await
        }
        // Pane kill confirm: X:<kill|keep>:<pane> — stateless buttons.
        ("X", Some(r)) => {
            if let Some((action, pane)) = r.split_once(':') {
                super::kill::handle_kill_action(&s, chat, msg_id, action, pane).await;
            }
        }
        _ => {}
    }
}
