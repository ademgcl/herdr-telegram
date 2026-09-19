use super::target::dm_pane;
use crate::{state::AppState, types::AgentRow};

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
    // Shell focus counts — facts cover every pane (rows omit shells).
    let focus_ws = match s.get_focus().await {
        Some(p) => match rows.iter().find(|r| r.pane == p).map(|r| r.ws.clone()) {
            Some(ws) => Some(ws),
            None => crate::herdr::labels::pane_facts(&s.cfg.socket)
                .await
                .ok()
                .and_then(|m| m.get(&p).map(|f| f.ws.clone())),
        },
        None => None,
    };
    let ws = if arg.is_empty() {
        focus_ws
    } else {
        Some(arg.to_string())
    };
    super::shell::open_shell(s, chat, None, ws.as_deref()).await;
}

pub(crate) async fn handle_pane(s: &AppState, chat: i64, arg: &str) {
    super::shell::open_pane_general(s, chat, None, arg).await;
}

pub(crate) async fn handle_split(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    arg: &str,
    reply_pane: &Option<String>,
) {
    // Direction words are never pane ids: `right|down` (or bare) splits
    // the reply/focus pane, anything else must resolve to a pane —
    // fail-closed via the same "who?" as /quit + /kill. A trailing
    // direction (`/split w1:p1 right`) splits that pane that way
    // (mirrors the `/read` trailing-count parse).
    let (pane_arg, dir) = match arg {
        "right" | "down" => ("", arg),
        _ => match arg.rsplit_once(char::is_whitespace) {
            Some((p, d)) if d == "right" || d == "down" => (p, d),
            _ => (arg, ""),
        },
    };
    let Some(pane) = super::target::dm_pane(s, rows, pane_arg, reply_pane).await else {
        s.tg.send_msg(
            chat,
            None,
            "who? `/split <pane>` or tap an agent in /agents",
            None,
        )
        .await;
        return;
    };
    super::shell::open_split(s, chat, None, &pane, dir).await;
}

pub(crate) async fn handle_space(s: &AppState, chat: i64, arg: &str) {
    let label = if super::space::check_label(arg) {
        arg.to_string()
    } else {
        super::space::next_label(s).await
    };
    super::space::open_space(s, chat, None, &label).await;
}
