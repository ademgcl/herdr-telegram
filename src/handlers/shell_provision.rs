use super::shell_common::shell_card_text;
use super::shell_run::run_shell_cmd;
use crate::{
    herdr::client::{create_tab, ensure_tg_space, list_workspaces},
    herdr::labels::pane_facts,
    state::AppState,
    ui::ws_label,
};

/// DM fallback: reply/focus may point at a shell pane (invisible to
/// agent.list) — run it as a command instead of asking "who?".
pub async fn run_shell_fallback(s: &AppState, chat: i64, reply: Option<String>, text: &str) {
    let pane = match reply {
        Some(p) => Some(p),
        None => s.get_focus().await,
    };
    match pane {
        Some(p) => run_shell_cmd(s, chat, None, &p, text).await,
        None => {
            s.tg.send_msg(
                chat,
                None,
                "who? tap an agent in /agents, or reply to its last message",
                None,
            )
            .await;
        }
    }
}

/// Open a fresh shell pane: new tab in `ws` (or the tg space), topic
/// badged shell, ready for commands. `ws` is a workspace id.
pub async fn open_shell(s: &AppState, chat: i64, thread: Option<i64>, ws: Option<&str>) {
    let ws_id = match ws {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => match ensure_tg_space(&s.cfg.socket).await {
            Ok(id) => id,
            Err(e) => {
                s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
                return;
            }
        },
    };
    let pane = match create_tab(&s.cfg.socket, &ws_id).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let space = ws_label(&spaces, &ws_id).to_string();
    // Kind "shell" mints an sh<n> tag; icon goes straight to shell.
    s.topics.sync_topic(&pane, "shell", &space, "shell").await;
    s.status
        .lock()
        .await
        .insert(pane.clone(), "shell".to_string());
    let mid =
        s.tg.send_msg(chat, thread, &shell_card_text(&pane), None)
            .await;
    s.remember(chat, mid, &pane).await;
    s.set_focus(&pane).await;
}

/// Split the pane sideways in the SAME tab: sibling shell pane + its own
/// topic, named/provisioned like any shell. `dir` is right|down.
pub async fn open_split(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, dir: &str) {
    let new = match crate::herdr::client::split_pane(&s.cfg.socket, pane, dir).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ split failed: {e}"), None)
                .await;
            return;
        }
    };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let ws_id = pane_facts(&s.cfg.socket)
        .await
        .ok()
        .and_then(|m| m.get(pane).map(|f| f.ws.clone()))
        .unwrap_or_default();
    let space = ws_label(&spaces, &ws_id).to_string();
    let space = if space.is_empty() { pane } else { &space };
    s.topics.sync_topic(&new, "shell", space, "shell").await;
    s.status
        .lock()
        .await
        .insert(new.clone(), "shell".to_string());
    let mid =
        s.tg.send_msg(chat, thread, &shell_card_text(&new), None)
            .await;
    s.remember(chat, mid, &new).await;
    s.set_focus(&new).await;
}
