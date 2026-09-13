use serde_json::Value;
use crate::{
    herdr::client::{
        list_agents, list_workspaces,
        read_agent_output, send_agent_keys,
    },
    jobs::enqueue_prompt,
    state::AppState,
    types::AgentRow,
    ui::{
        build_menu_text, help_text, main_menu_kb,
    },
};

pub async fn handle_dm_message(s: AppState, chat: i64, msg: &Value) {
    let text = msg["text"].as_str().unwrap_or("").trim();
    if text.is_empty() { return; }
    println!("[dm] from {chat}: {}", text.chars().take(40).collect::<String>());

    let (cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };

    let reply_pane: Option<String> = match msg["reply_to_message"]["message_id"].as_i64() {
        Some(rid) => s.targets.lock().await.get(&(chat, rid)).cloned(),
        None => None,
    };

    if cmd == "/start" || cmd == "/help" {
        s.tg.send_msg(chat, None, help_text(), None).await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&chat);
        s.runwait.lock().await.remove(&chat);
        s.typewait.lock().await.remove(&chat);
        let count = s.cancel_all_jobs().await;
        s.tg.send_msg(chat, None, &format!("✋ cancelled {count} pending job(s)"), None).await;
        return;
    }

    if let Some(ws) = s.runwait.lock().await.remove(&chat) {
        super::shell::handle_run_command(&s, chat, &ws, text).await;
        return;
    }

    if let Some(pane) = s.keywait.lock().await.remove(&chat) {
        let keys: Vec<&str> = text.split_whitespace().collect();
        match send_agent_keys(&s.cfg.socket, &pane, &keys).await {
            Ok(_) => { s.tg.send_msg(chat, None, "⌨️ sent", None).await; }
            Err(e) => { s.tg.send_msg(chat, None, &format!("⚠️ {e}"), None).await; }
        }
        return;
    }

    let rows = match list_agents(&s.cfg.socket).await {
        Ok(r) => r,
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("⚠️ herdr unreachable: {e}"), None).await;
            return;
        }
    };

    if cmd == "/agents" {
        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        s.tg.send_msg(chat, None, &build_menu_text(&spaces, &rows), Some(main_menu_kb(&spaces, &rows))).await;
        return;
    }

    if cmd == "/keys" && !arg.is_empty() {
        let (t, keys) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        let pane = if keys.is_empty() { None } else { resolve_target(&rows, Some(t)).map(|r| r.pane) };
        let Some(pane) = pane else {
            s.tg.send_msg(chat, None, "usage: /keys <pane|kind> <key> [key...]  e.g. /keys w8:p1 y enter", None).await;
            return;
        };
        let key_list: Vec<&str> = keys.split_whitespace().collect();
        match send_agent_keys(&s.cfg.socket, &pane, &key_list).await {
            Ok(_) => { s.tg.send_msg(chat, None, "⌨️ sent", None).await; }
            Err(e) => { s.tg.send_msg(chat, None, &format!("⚠️ {e}"), None).await; }
        }
        return;
    }

    if cmd == "/read" {
        let row = match resolve_target(&rows, Some(arg)) {
            Some(r) => Some(r),
            None if arg.is_empty() => s.get_focus().await.and_then(|p| rows.iter().find(|r| r.pane == p).cloned()),
            _ => None,
        };
        let Some(row) = row else {
            s.tg.send_msg(chat, None, "unknown target — see /agents", None).await;
            return;
        };
        match read_agent_output(&s.cfg.socket, &row.pane, 80).await {
            Ok(out) => {
                let body = if out.is_empty() { "(no output)".into() } else { out };
                let mid = s.tg.send_msg(chat, None, &body, None).await;
                s.remember(chat, mid, &row.pane).await;
                s.set_focus(&row.pane).await;
            }
            Err(e) => { s.tg.send_msg(chat, None, &format!("⚠️ {e}"), None).await; }
        }
        return;
    }

    if cmd == "/quit" {
        let Some(pane) = dm_pane(&s, &rows, arg).await else {
            s.tg.send_msg(chat, None, "who? `/quit <pane>` or tap an agent in /agents", None).await;
            return;
        };
        super::shell::quit_to_shell(&s, chat, None, &pane).await;
        return;
    }

    if cmd == "/kill" {
        let Some(pane) = dm_pane(&s, &rows, arg).await else {
            s.tg.send_msg(chat, None, "who? `/kill <pane>` or tap an agent in /agents", None).await;
            return;
        };
        super::kill::ask_kill(&s, chat, None, &pane).await;
        return;
    }

    if cmd == "/shell" {
        // No arg: shell next to the focused agent, else the tg space.
        let focus_ws = s.get_focus().await.and_then(|p| rows.iter().find(|r| r.pane == p).map(|r| r.ws.clone()));
        let ws = if arg.is_empty() { focus_ws } else { Some(arg.to_string()) };
        super::shell::open_shell(&s, chat, None, ws.as_deref()).await;
        return;
    }

    if cmd == "/model" {
        // `/model [target] [search]` — first token is a target only when it
        // resolves to a pane/kind; otherwise the whole arg is the search.
        let (mut pane, query) = match arg.split_once(char::is_whitespace) {
            Some((t, rest)) => match resolve_target(&rows, Some(t)) {
                Some(r) => (Some(r.pane), rest.trim()),
                None => (None, arg),
            },
            None => {
                if arg.is_empty() {
                    (None, "")
                } else if let Some(r) = resolve_target(&rows, Some(arg)) {
                    (Some(r.pane), "")
                } else {
                    (None, arg)
                }
            }
        };
        if pane.is_none()
            && let Some(p) = reply_pane.as_deref().and_then(|p| rows.iter().find(|r| r.pane == p))
        {
            pane = Some(p.pane.clone());
        }
        if pane.is_none()
            && let Some(f) = s.get_focus().await
            && rows.iter().any(|r| r.pane == f)
        {
            pane = Some(f);
        }
        if pane.is_none() {
            pane = resolve_target(&rows, Some("")).map(|r| r.pane);
        }
        let Some(pane) = pane else {
            s.tg.send_msg(chat, None, "who? `/model <pane>` or tap an agent in /agents", None).await;
            return;
        };
        if query.is_empty() {
            super::model::show_model(&s, chat, None, &pane).await;
        } else {
            let filter = super::model::search_filter(query);
            super::model::switch_by_filter(&s, chat, None, &pane, &filter, query).await;
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, None, "unknown command — /help", None).await;
        return;
    }

    // Answering a waiting prompt (set by the ⌨️ button on blocked cards).
    // Checked before routing: the next message belongs to the waiter.
    if let Some(wpane) = s.typewait.lock().await.remove(&chat) {
        match super::tap::type_text(&s, &wpane, text).await {
            Ok(()) => { s.tg.send_msg(chat, None, &format!("⌨️ typed into {wpane} + ⏎"), None).await; }
            Err(e) => { s.tg.send_msg(chat, None, &format!("⚠️ type failed: {e}"), None).await; }
        }
        return;
    }

    // Bare text prompt routing
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let explicit = if rest.is_empty() { None } else { resolve_target(&rows, Some(head)).map(|r| (r, rest.to_string())) };
    let via_reply = reply_pane.as_deref().and_then(|p| rows.iter().find(|r| r.pane == p)).cloned();
    let via_focus = s.get_focus().await.and_then(|p| rows.iter().find(|r| r.pane == p)).cloned();

    let (row, prompt_text) = if let Some(pair) = explicit {
        pair
    } else if let Some(r) = via_reply {
        (r, text.to_string())
    } else if let Some(r) = via_focus {
        (r, text.to_string())
    } else if let Some(r) = resolve_target(&rows, Some("")) {
        (r, text.to_string())
    } else {
        // Reply/focus may point at a shell pane (invisible to agent.list).
        super::shell::run_shell_fallback(&s, chat, reply_pane.clone(), text).await;
        return;
    };

    // Blocked panes reject text prompts — type into the waiting prompt.
    if row.status == "blocked" {
        s.set_focus(&row.pane).await;
        match super::tap::type_text(&s, &row.pane, &prompt_text).await {
            Ok(()) => { s.tg.send_msg(chat, None, &format!("⌨️ typed into {} + ⏎", row.pane), None).await; }
            Err(_) => { super::dialog::send_blocked_card(&s, chat, None, &row.pane).await; }
        }
        return;
    }
    s.set_focus(&row.pane).await;
    enqueue_prompt(s.clone(), chat, None, row, prompt_text).await;
}

pub fn resolve_target(rows: &[AgentRow], spec: Option<&str>) -> Option<AgentRow> {
    match spec {
        None | Some("") => {
            if rows.len() == 1 { rows.first().cloned() } else { None }
        }
        Some(t) => rows.iter().find(|r| r.pane == t).cloned().or_else(|| {
            let m: Vec<_> = rows.iter().filter(|r| r.kind == t).collect();
            if m.len() == 1 { m.first().map(|r| (*r).clone()) } else { None }
        }),
    }
}

/// DM pane target: explicit pane/kind, else focus (which may be a rowless
/// shell pane — callers report that case themselves).
async fn dm_pane(s: &AppState, rows: &[AgentRow], arg: &str) -> Option<String> {
    match resolve_target(rows, Some(arg)) {
        Some(r) => Some(r.pane),
        None if arg.is_empty() => s.get_focus().await,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_target_by_pane() {
        let rows = vec![
            AgentRow {
                kind: "claude".into(),
                pane: "w1:p1".into(),
                title: "dev".into(),
                status: "idle".into(),
                ws: "w1".into(),
            },
            AgentRow {
                kind: "opencode".into(),
                pane: "w1:p2".into(),
                title: "fix".into(),
                status: "working".into(),
                ws: "w1".into(),
            },
        ];
        let found = resolve_target(&rows, Some("w1:p1"));
        assert_eq!(found.unwrap().pane, "w1:p1");

        let by_kind = resolve_target(&rows, Some("opencode"));
        assert_eq!(by_kind.unwrap().pane, "w1:p2");

        let nonexistent = resolve_target(&rows, Some("gemini"));
        assert!(nonexistent.is_none());
    }

    #[test]
    fn test_resolve_single_agent() {
        let rows = vec![AgentRow {
            kind: "claude".into(),
            pane: "w1:p1".into(),
            title: "".into(),
            status: "idle".into(),
            ws: "w1".into(),
        }];
        let found = resolve_target(&rows, None);
        assert_eq!(found.unwrap().pane, "w1:p1");
    }
}
