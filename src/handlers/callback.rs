use std::time::{SystemTime, UNIX_EPOCH};
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
        println!("[callback] ignoring non-owner tap from {from}");
        return;
    }
    let chat = cbq["message"]["chat"]["id"].as_i64();
    let msg_id = cbq["message"]["message_id"].as_i64();
    let data = cbq["data"].as_str().unwrap_or("");

    if let Some(cbq_id) = cbq["id"].as_str() {
        s.tg.answer_callback(cbq_id).await;
    }
    let (Some(chat), Some(msg_id)) = (chat, msg_id) else { return };

    // Thread for ack messages (forum topics carry it, DMs don't).
    let thread = cbq["message"]["message_thread_id"].as_i64();
    let date = cbq["message"]["date"].as_u64().unwrap_or(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
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
            s.tg.edit_msg(chat, msg_id, "spawn which agent?", Some(spawn_kb(None))).await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("n", Some(ws)) => {
            s.tg.edit_msg(chat, msg_id, &format!("spawn into {ws}: which agent?"), Some(spawn_kb(Some(ws)))).await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("N", None) => handle_new_space(&s, chat, msg_id, thread).await,
        ("k", Some(r)) => match r.split_once(':') {
            Some((ws, kind)) => handle_spawn(&s, chat, msg_id, kind, Some(ws)).await,
            None => handle_spawn(&s, chat, msg_id, r, None).await,
        },
        ("K", Some(pane)) => {
            s.keywait.lock().await.insert((chat, thread), pane.to_string());
            s.set_focus(pane).await;
            s.tg.edit_msg(chat, msg_id, &format!("⌨️ send keys for {pane}\nnext message = keys (e.g. `y enter`, `esc`)"), None).await;
        }
        ("R", Some(ws)) => {
            s.runwait.lock().await.insert((chat, thread), ws.to_string());
            s.tg.edit_msg(chat, msg_id, &format!("⌨️ send shell command for {ws}\nnext message = command"), None).await;
        }
        ("p", Some(pane)) => {
            let out = read_pane_output(&s.cfg.socket, pane, 120).await.unwrap_or_default();
            let body = if out.is_empty() { "(no output)".into() } else { out };
            let mid = s.tg.send_msg(chat, thread, &body, Some(pane_output_kb(pane))).await;
            s.remember(chat, mid, pane).await;
        }
        ("m", None) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            s.tg.edit_msg(chat, msg_id, &build_menu_text(&spaces, &agents), Some(main_menu_kb(&spaces, &agents))).await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("w", Some(ws)) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
            let my_agents: Vec<_> = agents.into_iter().filter(|a| a.ws == *ws).collect();
            s.tg.edit_msg(chat, msg_id, &build_ws_text(ws, &spaces, &my_agents), Some(workspace_kb(ws, &my_agents))).await;
            s.targets.lock().await.remove(&(chat, msg_id));
        }
        ("a", Some(pane)) => {
            s.remember(chat, Some(msg_id), pane).await;
            s.set_focus(pane).await;
            if let Ok(agent) = get_agent(&s.cfg.socket, pane).await {
                s.tg.edit_msg(chat, msg_id, &build_agent_card_text(&agent), Some(agent_card_kb(pane, &agent.ws))).await;
            }
        }
        ("o", Some(pane)) => {
            let out = read_agent_output(&s.cfg.socket, pane, 120).await.unwrap_or_default();
            let body = if out.is_empty() { "(no output)".into() } else { out };
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
        ("M", Some(r)) => handle_model_tap(&s, chat, msg_id, thread, r).await,
        // Pane kill confirm: X:<kill|keep>:<pane> — stateless buttons.
        ("X", Some(r)) => {
            if let Some((action, pane)) = r.split_once(':') {
                super::kill::handle_kill_action(&s, chat, msg_id, action, pane).await;
            }
        }
        _ => {}
    }
}

async fn handle_new_space(s: &AppState, chat: i64, msg_id: i64, thread: Option<i64>) {
    s.tg.edit_msg(chat, msg_id, "⏳ creating space…", None).await;
    let label = super::space::next_label(s).await;
    let ws_id = match create_workspace(&s.cfg.socket, &label).await {
        Ok(id) => id,
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ failed to create space: {e}"), None).await;
            return;
        }
    };
    super::shell::open_shell(s, chat, thread, Some(&ws_id)).await;
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
    s.tg.edit_msg(chat, msg_id, &build_menu_text(&spaces, &agents), Some(main_menu_kb(&spaces, &agents))).await;
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
                    s.topics.sync_topic(&agent.pane, &agent.kind, sp, &agent.status).await;
                }
                s.tg.edit_msg(chat, msg_id, &build_agent_card_text(&agent), Some(agent_card_kb(&row.pane, &agent.ws))).await;
            }
        }
        Err(e) => {
            s.tg.edit_msg(chat, msg_id, &format!("⚠️ spawn failed: {e}"), None).await;
        }
    }
}

/// Model-card taps: `M:list:<pane>` re-renders the card in place,
/// `M:<idx>:<pane>` switches to that free-Zen model with progress edits.
async fn handle_model_tap(s: &AppState, chat: i64, msg_id: i64, _thread: Option<i64>, r: &str) {
    let Some((idx, pane)) = r.split_once(':') else { return };
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
    let Some((filter, marker)) = super::model_parse::free_tap(i) else { return };
    s.set_focus(pane).await;
    // Strip the buttons while switching: mid-switch taps can only collide.
    let no_kb = Some(Value::Array(Vec::new()));
    s.tg.edit_msg(chat, msg_id, &format!("⏳ switching {pane} → `{marker}`…"), no_kb.clone()).await;
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
