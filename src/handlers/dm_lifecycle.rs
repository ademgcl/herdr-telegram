use super::target::dm_pane;
use crate::{
    herdr::client::{list_workspaces, spawn_agent},
    state::AppState,
    types::AgentRow,
    ui::ws_label,
};

pub(crate) async fn handle_quit(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    let Some(pane) = dm_pane(s, rows, arg, reply_pane).await else {
        s.tg.send_msg(
            chat,
            None,
            "who? `/quit <pane>` or tap an agent in /agents",
            None,
        )
        .await;
        return;
    };
    super::shell::quit_to_shell(s, chat, None, &pane).await;
}

pub(crate) async fn handle_kill(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    let Some(pane) = dm_pane(s, rows, arg, reply_pane).await else {
        s.tg.send_msg(
            chat,
            None,
            "who? `/kill <pane>` or tap an agent in /agents",
            None,
        )
        .await;
        return;
    };
    super::kill::ask_kill(s, chat, None, &pane).await;
}

pub(crate) async fn handle_shell(s: &AppState, chat: i64, rows: &[AgentRow], arg: &str) {
    // No arg: shell next to the focused agent, else the tg space.
    let focus_ws = s
        .get_focus()
        .await
        .and_then(|p| rows.iter().find(|r| r.pane == p).map(|r| r.ws.clone()));
    let ws = if arg.is_empty() {
        focus_ws
    } else {
        Some(arg.to_string())
    };
    super::shell::open_shell(s, chat, None, ws.as_deref()).await;
}

pub(crate) async fn handle_space(s: &AppState, chat: i64, arg: &str) {
    let label = if super::space::check_label(arg) {
        arg.to_string()
    } else {
        super::space::next_label(s).await
    };
    super::space::open_space(s, chat, None, &label).await;
}

pub(crate) async fn handle_spawn(s: &AppState, chat: i64, arg: &str) {
    let (kind, ws) = match arg.split_once(char::is_whitespace) {
        Some((k, w)) => (k, Some(w)),
        None if !arg.is_empty() => (arg, None),
        _ => {
            s.tg.send_msg(
                chat,
                None,
                "usage: /spawn <kind> [space] (e.g. /spawn opencode space-1)",
                None,
            )
            .await;
            return;
        }
    };
    s.tg.send_msg(chat, None, &format!("spawning {kind}..."), None)
        .await;
    // Workspace may be an id or a human label (`space-1`).
    let ws_id;
    let ws = match ws {
        Some(w) => match super::space::resolve_ws(s, w).await {
            Some(id) => {
                ws_id = id;
                Some(ws_id.as_str())
            }
            None => {
                s.tg.send_msg(chat, None, &format!("⚠️ unknown space `{w}`"), None)
                    .await;
                return;
            }
        },
        None => None,
    };
    match spawn_agent(&s.cfg.socket, kind, ws).await {
        Ok(row) => {
            let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
            let space = ws_label(&spaces, &row.ws);
            if let Some(topic_th) = s
                .topics
                .sync_topic(&row.pane, &row.kind, space, &row.status)
                .await
            {
                s.tg.send_msg(
                    chat,
                    None,
                    &format!("Started {} [{}] in topic #{topic_th}", row.kind, row.pane),
                    None,
                )
                .await;
            } else {
                s.tg.send_msg(
                    chat,
                    None,
                    &format!("Started {} [{}]", row.kind, row.pane),
                    None,
                )
                .await;
            }
            s.set_focus(&row.pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, None, &format!("spawn failed: {e}"), None)
                .await;
        }
    }
}
