use super::shell_common::shell_card_text;
use super::shell_run::run_shell_cmd;
use crate::{
    herdr::client::{
        await_fresh_root, create_tab, ensure_tg_space, get_agent, list_workspaces,
    },
    herdr::labels::pane_facts,
    jobs::enqueue_prompt,
    state::AppState,
    types::AgentRow,
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
        Some(p) => {
            // A live agent here takes prompts, not shell input: stale
            // rows can omit fresh agents, and shell text must never be
            // injected into an agent session.
            match get_agent(&s.cfg.socket, &p).await {
                Ok(a) => {
                    enqueue_prompt(
                        s.clone(),
                        chat,
                        None,
                        AgentRow {
                            kind: a.kind,
                            pane: a.pane,
                            title: a.title,
                            status: a.status,
                            ws: a.ws,
                        },
                        text.to_string(),
                    )
                    .await;
                }
                Err(_) => run_shell_cmd(s, chat, None, &p, text).await,
            }
        }
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
/// badged shell, ready for commands. `ws` is a workspace id or label.
/// A just-created `tg` space reuses its root pane (else p1 orphans);
/// existing spaces always get a new tab (reuse would hijack live panes).
pub async fn open_shell(s: &AppState, chat: i64, thread: Option<i64>, ws: Option<&str>) {
    let (ws_id, reuse) = match ws {
        Some(w) if !w.is_empty() => match super::space::resolve_ws(s, w).await {
            Some(id) => (id, None),
            None => {
                s.tg.send_msg(chat, thread, &format!("⚠️ unknown space `{w}`"), None)
                    .await;
                return;
            }
        },
        _ => match ensure_tg_space(&s.cfg.socket).await {
            Ok((id, true)) => {
                let reuse = await_fresh_root(&s.cfg.socket, &id).await;
                (id, reuse)
            }
            Ok((id, false)) => (id, None),
            Err(e) => {
                s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
                return;
            }
        },
    };
    if let Some(pane) = reuse {
        attach_shell_pane(s, chat, thread, &pane, &space_label(s, &ws_id).await).await;
        return;
    }
    create_tab_and_attach(s, chat, thread, &ws_id).await;
}

/// Fresh space → shell topic WITHOUT a second tab: `ws_id` was just
/// created, so its root pane is ours — reuse it. Retries the lookup
/// (create may materialize the pane a beat late); falls back to
/// `tab.create` with the KNOWN id (never re-resolve: a just-created
/// id may not list yet, which must not read as "unknown space").
pub async fn open_space_shell(s: &AppState, chat: i64, thread: Option<i64>, ws_id: &str) {
    if ws_id.is_empty() {
        s.tg.send_msg(chat, thread, "⚠️ space create returned no id", None)
            .await;
        return;
    }
    match await_fresh_root(&s.cfg.socket, ws_id).await {
        Some(pane) => attach_shell_pane(s, chat, thread, &pane, &space_label(s, ws_id).await).await,
        None => create_tab_and_attach(s, chat, thread, ws_id).await,
    }
}

/// Fresh tab in a known workspace id + shell badge. The id is already
/// resolved — never `resolve_ws` here (fresh ids may not list yet).
async fn create_tab_and_attach(s: &AppState, chat: i64, thread: Option<i64>, ws_id: &str) {
    let pane = match create_tab(&s.cfg.socket, ws_id).await {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            s.tg.send_msg(chat, thread, "⚠️ tab.create returned no pane", None)
                .await;
            return;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {e}"), None).await;
            return;
        }
    };
    attach_shell_pane(s, chat, thread, &pane, &space_label(s, ws_id).await).await;
}

async fn space_label(s: &AppState, ws_id: &str) -> String {
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    ws_label(&spaces, ws_id).to_string()
}

/// Badge an existing pane as a shell: topic + status + card + focus.
/// Shared by fresh tabs, reused space roots, and splits. Takes the
/// display label (splits fall back to the pane when the space is gone).
async fn attach_shell_pane(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, space: &str) {
    // Kind "shell" mints an sh<n> tag; icon goes straight to shell.
    s.topics.sync_topic(pane, "shell", space).await;
    s.status
        .lock()
        .await
        .insert(pane.to_string(), "shell".to_string());
    let mid = s.tg.send_msg(chat, thread, &shell_card_text(pane), None).await;
    s.remember(chat, mid, pane).await;
    s.set_focus(pane).await;
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
    attach_shell_pane(s, chat, thread, &new, space).await;
}
