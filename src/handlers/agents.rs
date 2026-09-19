//! Shared `/agents` panel + `/spawn` flow: General, DM-adjacent forum
//! topics and shell topics all serve the same control-plane reads/writes,
//! so one source (no per-surface drift). Topics pass their own thread so
//! the panel/ack lands where it was asked — Telegram menus are
//! chat-global, a redirect there only reads as broken.
use crate::{
    herdr::client::{list_agents, list_workspaces, spawn_agent},
    state::AppState,
    ui::{build_menu_text, main_menu_kb, open_topic_kb, ws_label},
};

/// Read-only panel: spaces + agents + spawn buttons. No focus change.
pub(crate) async fn show_panel(s: &AppState, chat: i64, thread: Option<i64>) {
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
    s.tg.send_msg(
        chat,
        thread,
        &build_menu_text(&spaces, &agents),
        Some(main_menu_kb(&spaces, &agents)),
    )
    .await;
}

/// Dispatcher for topic routers (one arm, two commands): keeps the
/// 300-line routers small — `/agents` panels, `/spawn` spawns.
/// Fail-closed: anything else writes nothing (callers pre-gate, this
/// is the backstop).
pub(crate) async fn handle_control(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    cmd: &str,
    arg: &str,
) {
    if cmd == "/agents" {
        show_panel(s, chat, thread).await;
    } else if cmd == "/spawn" {
        spawn_with_arg(s, chat, thread, arg).await;
    }
    // Unknown cmds (should not reach here — callers pre-gate) write nothing.
}
/// Spawn `<kind> [space]` + mint its topic. Explicit-arg write only —
/// no pane context, no misroute. Takes focus like `/shell` from topics.
pub(crate) async fn spawn_with_arg(s: &AppState, chat: i64, thread: Option<i64>, arg: &str) {
    let (kind, ws) = match arg.split_once(char::is_whitespace) {
        Some((k, w)) => (k, Some(w)),
        None if !arg.is_empty() => (arg, None),
        _ => {
            s.tg.send_msg(
                chat,
                thread,
                "usage: `/spawn <kind> [space]` (e.g. `/spawn opencode space-1`)",
                None,
            )
            .await;
            return;
        }
    };
    s.tg.send_msg(chat, thread, &format!("⏳ spawning {kind}…"), None)
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
                s.tg.send_msg(chat, thread, &crate::ui::unknown_space(w), None)
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
            // Fresh pane: no dialog generation can exist, but a raced
            // prune still retires (uniform with every sync site).
            let (topic_th, pruned) = s.topics.sync_topic_prune(&row.pane, &row.kind, space).await;
            if pruned {
                crate::handlers::dialog::retire_dialog(s, &row.pane).await;
            }
            if let Some(topic_th) = topic_th {
                // One-tap jump like the shell opener (deep link beats a
                // bare thread number).
                let kb = open_topic_kb(chat, topic_th);
                s.tg.send_msg(
                    chat,
                    thread,
                    &format!(
                        "✅ Started {} [{}] in topic #{topic_th}",
                        row.kind, row.pane
                    ),
                    kb,
                )
                .await;
            } else {
                s.tg.send_msg(
                    chat,
                    thread,
                    &format!("✅ Started {} [{}]", row.kind, row.pane),
                    None,
                )
                .await;
            }
            s.set_focus(&row.pane).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &crate::ui::spawn_failed(&e.to_string()), None)
                .await;
        }
    }
}
