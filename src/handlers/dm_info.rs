use super::target::resolve_target;
use crate::{
    herdr::client::{get_agent, list_workspaces, read_agent_output, send_agent_keys},
    state::AppState,
    types::AgentRow,
    ui::{agent_card_kb, build_agent_card_text, build_menu_text, main_menu_kb},
};

pub(crate) async fn handle_agents(s: &AppState, chat: i64, rows: &[AgentRow]) {
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    s.tg.send_msg(
        chat,
        None,
        &build_menu_text(&spaces, rows),
        Some(main_menu_kb(&spaces, rows)),
    )
    .await;
}

pub(crate) async fn handle_keys(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    let (t, keys) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
    // Explicit `<pane|kind> <keys...>`, else the full arg is keys for the
    // replied-to card. A stale reply never silently reroutes to a
    // different agent: shell/dead panes fail visibly inside send_keys.
    // An explicit pane with no keys is a usage error, not keys for the
    // reply (typo guard).
    let (pane, keys) = match resolve_target(rows, Some(t)) {
        Some(r) if !keys.is_empty() => (Some(r.pane), keys),
        Some(_) => (None, keys),
        None if reply_pane.is_some() && !arg.is_empty() => (reply_pane.clone(), arg),
        _ => (None, keys),
    };
    let Some(pane) = pane else {
        s.tg.send_msg(
            chat,
            None,
            "usage: /keys <pane|kind> <key> [key...]  e.g. /keys w8:p1 y enter",
            None,
        )
        .await;
        return;
    };
    send_keys(s, chat, &pane, keys).await;
}

async fn send_keys(s: &AppState, chat: i64, pane: &str, keys: &str) {
    let key_list: Vec<&str> = keys.split_whitespace().collect();
    match send_agent_keys(&s.cfg.socket, pane, &key_list).await {
        Ok(_) => {
            s.tg.send_msg(chat, None, "⌨️ sent", None).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("⚠️ {e}"), None).await;
        }
    }
}

pub(crate) async fn handle_read(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    // Target order matches /status: explicit arg, else replied-to card,
    // else live focus, else the sole agent. An explicit but unknown arg
    // errors — it must never answer for a different agent.
    let mut row = match resolve_target(rows, if arg.is_empty() { None } else { Some(arg) }) {
        Some(r) => Some(r),
        None if !arg.is_empty() => {
            s.tg.send_msg(chat, None, "unknown target — see /agents", None)
                .await;
            return;
        }
        None => reply_pane
            .as_deref()
            .and_then(|p| rows.iter().find(|r| r.pane == p).cloned()),
    };
    if row.is_none() {
        if let Some(f) = s
            .get_focus()
            .await
            .filter(|f| rows.iter().any(|r| &r.pane == f))
        {
            row = rows.iter().find(|r| r.pane == f).cloned();
        } else {
            row = resolve_target(rows, Some(""));
        }
    }
    let Some(row) = row else {
        s.tg.send_msg(chat, None, "unknown target — see /agents", None)
            .await;
        return;
    };
    match read_agent_output(&s.cfg.socket, &row.pane, 80).await {
        Ok(out) => {
            let body = if out.is_empty() {
                "(no output)".into()
            } else {
                out
            };
            let mid = s.tg.send_msg(chat, None, &body, None).await;
            s.remember(chat, mid, &row.pane).await;
            s.set_focus(&row.pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("⚠️ {e}"), None).await;
        }
    }
}

pub(crate) async fn handle_status(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    let mut pane = match resolve_target(rows, if arg.is_empty() { None } else { Some(arg) }) {
        Some(r) => Some(r.pane),
        // Explicit but unknown: never show a different agent's card.
        None if !arg.is_empty() => {
            s.tg.send_msg(chat, None, "unknown target — see /agents", None)
                .await;
            return;
        }
        None => reply_pane.clone(),
    };
    if pane.is_none() {
        if let Some(f) = s
            .get_focus()
            .await
            .filter(|f| rows.iter().any(|r| &r.pane == f))
        {
            pane = Some(f);
        } else {
            pane = resolve_target(rows, Some("")).map(|r| r.pane);
        }
    }
    let Some(pane) = pane else {
        s.tg.send_msg(chat, None, "unknown target — see /agents", None)
            .await;
        return;
    };
    match get_agent(&s.cfg.socket, &pane).await {
        Ok(agent) => {
            let mid =
                s.tg.send_msg(
                    chat,
                    None,
                    &build_agent_card_text(&agent),
                    Some(agent_card_kb(&pane, &agent.ws)),
                )
                .await;
            s.remember(chat, mid, &pane).await;
            s.set_focus(&pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("status failed: {e}"), None)
                .await;
        }
    }
}
