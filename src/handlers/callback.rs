use serde_json::Value;
use crate::{
    herdr::client::{
        create_workspace, get_agent, list_agents, list_workspaces,
        read_agent_output, read_pane_output, spawn_agent,
    },
    state::AppState,
    ui::{
        agent_card_kb, btn, build_agent_card_text, build_menu_text,
        build_ws_text, main_menu_kb, pane_output_kb, spawn_kb, workspace_kb,
    },
};

pub async fn handle_callback(s: AppState, cbq: &Value) {
    let Some(from) = cbq["from"]["id"].as_i64() else { return };
    if !s.cfg.owners.contains(&from) {
        return;
    }
    let chat = cbq["message"]["chat"]["id"].as_i64();
    let msg_id = cbq["message"]["message_id"].as_i64();
    let data = cbq["data"].as_str().unwrap_or("");

    if let Some(cbq_id) = cbq["id"].as_str() {
        s.tg.answer_callback(cbq_id).await;
    }
    let (Some(chat), Some(msg_id)) = (chat, msg_id) else { return };

    let route: Vec<&str> = data.splitn(3, ':').collect();
    match route.as_slice() {
        ["n"] => {
            s.tg.edit_msg(chat, msg_id, "spawn which agent?", Some(spawn_kb(None))).await;
        }
        ["n", ws] => {
            s.tg.edit_msg(chat, msg_id, &format!("spawn into {ws}: which agent?"), Some(spawn_kb(Some(ws)))).await;
        }
        ["N"] => handle_new_space(&s, chat, msg_id).await,
        ["k", kind] => handle_spawn(&s, chat, msg_id, kind, None).await,
        ["k", ws, kind] => handle_spawn(&s, chat, msg_id, kind, Some(ws)).await,
        ["K", pane] => {
            s.keywait.lock().await.insert(chat, pane.to_string());
            s.set_focus(pane).await;
            s.tg.edit_msg(chat, msg_id, &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"), None).await;
        }
        ["R", ws] => {
            s.runwait.lock().await.insert(chat, ws.to_string());
            s.tg.edit_msg(chat, msg_id, &format!("⌨️ send shell command for {ws}\nnext message = command"), None).await;
        }
        ["p", pane] => {
            let out = read_pane_output(&s.cfg.socket, pane, 120).await.unwrap_or_default();
            let body = if out.is_empty() { "(no output)".into() } else { out };
            s.tg.send_msg(chat, None, &body, Some(pane_output_kb(pane))).await;
        }
        ["m"] => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            s.tg.edit_msg(chat, msg_id, &build_menu_text(&spaces, &agents), Some(main_menu_kb(&spaces, &agents))).await;
        }
        ["w", ws] => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            let my_agents: Vec<_> = agents.into_iter().filter(|a| a.ws == *ws).collect();
            s.tg.edit_msg(chat, msg_id, &build_ws_text(ws, &spaces, &my_agents), Some(workspace_kb(ws, &my_agents))).await;
        }
        ["a", pane] => {
            s.remember(chat, Some(msg_id), pane).await;
            s.set_focus(pane).await;
            if let Ok(agent) = get_agent(&s.cfg.socket, pane).await {
                s.tg.edit_msg(chat, msg_id, &build_agent_card_text(&agent), Some(agent_card_kb(pane, &agent.ws))).await;
            }
        }
        ["o", pane] => {
            let out = read_agent_output(&s.cfg.socket, pane, 120).await.unwrap_or_default();
            let body = if out.is_empty() { "(no output)".into() } else { out };
            let kb = serde_json::json!([[btn("← back", &format!("a:{pane}"))]]);
            let mid = s.tg.send_msg(chat, None, &body, Some(kb)).await;
            s.remember(chat, mid, pane).await;
            s.set_focus(pane).await;
        }
        _ => {}
    }
}

async fn handle_new_space(s: &AppState, chat: i64, msg_id: i64) {
    s.tg.edit_msg(chat, msg_id, "⏳ creating space…", None).await;
    let n = list_workspaces(&s.cfg.socket).await.map(|w| w.len()).unwrap_or(0) + 1;
    match create_workspace(&s.cfg.socket, &format!("space-{n}")).await {
        Ok(_) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            s.tg.edit_msg(chat, msg_id, &build_menu_text(&spaces, &agents), Some(main_menu_kb(&spaces, &agents))).await;
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ failed to create space: {e}"), None).await;
        }
    }
}

async fn handle_spawn(s: &AppState, chat: i64, msg_id: i64, kind: &str, ws: Option<&str>) {
    let label = ws.unwrap_or("tg");
    s.tg.edit_msg(chat, msg_id, &format!("⏳ starting {kind} in {label} space…"), None).await;
    match spawn_agent(&s.cfg.socket, kind, ws).await {
        Ok(row) => {
            s.remember(chat, Some(msg_id), &row.pane).await;
            s.set_focus(&row.pane).await;
            if let Ok(agent) = get_agent(&s.cfg.socket, &row.pane).await {
                // Ensure topic exists in forum group if enabled
                if s.cfg.forum.is_some() {
                    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
                    let sp = spaces.iter().find(|w| w.id == agent.ws).map(|w| w.label.as_str()).unwrap_or(&agent.ws);
                    s.topics.ensure_topic(&agent.pane, &agent.kind, sp, &agent.status).await;
                }
                s.tg.edit_msg(chat, msg_id, &build_agent_card_text(&agent), Some(agent_card_kb(&row.pane, &agent.ws))).await;
            }
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ spawn failed: {e}"), None).await;
        }
    }
}
