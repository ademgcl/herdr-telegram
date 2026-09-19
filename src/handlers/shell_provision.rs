use super::shell_common::shell_card_text;
use super::shell_run::run_shell_cmd;
use crate::{
    herdr::client::{
        await_fresh_root, best_split_direction, create_tab, ensure_tg_space, get_agent, list_agents,
        list_workspaces, pane_layout,
    },
    herdr::labels::pane_facts,
    jobs::enqueue_prompt,
    state::AppState,
    types::AgentRow,
    ui::{open_topic_kb, ws_label},
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
            // injected into an agent session. Fail-closed: an unreadable
            // agent row is confirmed via `agent.list` (a blip retries
            // instead of injecting shell text into live work; a
            // confirmed-shell pane runs as a command below).
            let row = match get_agent(&s.cfg.socket, &p).await {
                Ok(a) => Some(AgentRow {
                    kind: a.kind,
                    pane: a.pane,
                    title: a.title,
                    status: a.status,
                    ws: a.ws,
                }),
                Err(_) => match list_agents(&s.cfg.socket).await {
                    Err(_) => {
                        s.tg.send_msg(chat, None, crate::ui::scope_text::HERDR_RETRY, None)
                            .await;
                        return;
                    }
                    Ok(rows) => rows.into_iter().find(|r| r.pane == p),
                },
            };
            match row {
                Some(row) => {
                    enqueue_prompt(s.clone(), chat, None, row, text.to_string()).await;
                }
                None => run_shell_cmd(s, chat, None, &p, text).await,
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
    open_shell_inner(s, chat, thread, ws, true).await;
}

/// `/pane [space]`: same opener as a sidecar — new shell pane + topic,
/// card + open-topic button, focus stays where it is (like /split,
/// unlike /shell). `ws_id` is already resolved.
async fn open_pane(s: &AppState, chat: i64, thread: Option<i64>, ws_id: &str) {
    if ws_id.is_empty() {
        s.tg.send_msg(chat, thread, crate::ui::SPACE_UNREADABLE, None).await;
        return;
    }
    create_tab_and_attach(s, chat, thread, ws_id, false).await;
}

/// Topic `/pane [space]`: this pane's space, or the named one.
/// Fail-closed: an unreadable space never opens in the wrong one.
pub async fn open_pane_here(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, arg: &str) {
    if !arg.is_empty() {
        match super::space::resolve_ws(s, arg).await {
            Some(id) => open_pane(s, chat, thread, &id).await,
            None => {
                s.tg.send_msg(chat, thread, &format!("⚠️ unknown space `{arg}` — see `/agents`"), None).await;
            }
        }
        return;
    }
    match pane_facts(&s.cfg.socket).await.ok().and_then(|m| m.get(pane).map(|f| f.ws.clone())) {
        Some(ws) if !ws.is_empty() => open_pane(s, chat, thread, &ws).await,
        _ => {
            s.tg.send_msg(chat, thread, crate::ui::SPACE_UNREADABLE, None).await;
        }
    }
}

/// General/DM `/pane [space]`: named space, else the focused pane's
/// space (shell focus counts — facts cover every pane), else tg.
pub async fn open_pane_general(s: &AppState, chat: i64, thread: Option<i64>, arg: &str) {
    if !arg.is_empty() {
        match super::space::resolve_ws(s, arg).await {
            Some(id) => open_pane(s, chat, thread, &id).await,
            None => {
                s.tg.send_msg(chat, thread, &format!("⚠️ unknown space `{arg}` — see `/agents`"), None).await;
            }
        }
        return;
    }
    let ws = match s.get_focus().await {
        Some(p) => pane_facts(&s.cfg.socket).await.ok().and_then(|m| m.get(&p).map(|f| f.ws.clone())).unwrap_or_default(),
        None => String::new(),
    };
    if ws.is_empty() {
        open_shell_inner(s, chat, thread, None, false).await;
        return;
    }
    open_pane(s, chat, thread, &ws).await;
}

async fn open_shell_inner(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    ws: Option<&str>,
    follow: bool,
) {
    let (ws_id, reuse) = match ws {
        Some(w) if !w.is_empty() => match super::space::resolve_ws(s, w).await {
            Some(id) => (id, None),
            None => {
                s.tg.send_msg(chat, thread, &format!("⚠️ unknown space `{w}` — see `/agents`"), None)
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
                s.tg.send_msg(chat, thread, &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None).await;
                return;
            }
        },
    };
    if let Some(pane) = reuse {
        attach_shell_pane(s, chat, thread, &pane, &space_label(s, &ws_id).await, follow).await;
        return;
    }
    create_tab_and_attach(s, chat, thread, &ws_id, follow).await;
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
        Some(pane) => {
            attach_shell_pane(s, chat, thread, &pane, &space_label(s, ws_id).await, true).await
        }
        None => create_tab_and_attach(s, chat, thread, ws_id, true).await,
    }
}

/// Fresh tab in a known workspace id + shell badge. The id is already
/// resolved — never `resolve_ws` here (fresh ids may not list yet).
async fn create_tab_and_attach(s: &AppState, chat: i64, thread: Option<i64>, ws_id: &str, follow: bool) {
    let pane = match create_tab(&s.cfg.socket, ws_id).await {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            s.tg.send_msg(chat, thread, "⚠️ tab.create returned no pane", None)
                .await;
            return;
        }
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None).await;
            return;
        }
    };
    attach_shell_pane(s, chat, thread, &pane, &space_label(s, ws_id).await, follow).await;
}

async fn space_label(s: &AppState, ws_id: &str) -> String {
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    ws_label(&spaces, ws_id).to_string()
}

/// Badge an existing pane as a shell: topic + status + card + focus.
/// The reply carries a one-tap open-topic button (deep link) instead of
/// relying on a pin inside the new topic. Shared by fresh tabs, reused
/// space roots, and splits. Takes the display label (splits fall back to
/// the pane when the space is gone).
async fn attach_shell_pane(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    space: &str,
    follow_focus: bool,
) {
    // Kind "shell" mints an sh<n> tag; icon goes straight to shell.
    // Fresh pane: retire only on a raced prune (uniform, see agents.rs).
    let topic = match s.topics.sync_topic_prune(pane, "shell", space).await {
        (t, true) => {
            crate::handlers::dialog::retire_dialog(s, pane).await;
            t
        }
        (t, false) => t,
    };
    s.status
        .lock()
        .await
        .insert(pane.to_string(), "shell".to_string());
    let kb = topic.and_then(|th| open_topic_kb(chat, th));
    let mid =
        s.tg.send_msg(chat, thread, &shell_card_text(pane), kb)
            .await;
    s.remember(chat, mid, pane).await;
    // Explicit shells take focus; side splits must not steal global
    // DM/General routing from live work (topics route by thread anyway).
    if follow_focus {
        s.set_focus(pane).await;
    }
}

/// Split the pane sideways in the SAME tab: sibling shell pane + its own
/// topic, named/provisioned like any shell. Empty `dir` picks the pane's
/// longer axis from live layout (explicit right|down always wins);
/// unreadable layout falls back to right.
pub async fn open_split(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, dir: &str) {
    let dir = if dir.is_empty() {
        auto_split_dir(&s.cfg.socket, pane).await
    } else {
        dir.to_string()
    };
    let new = match crate::herdr::client::split_pane(&s.cfg.socket, pane, &dir).await {
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(chat, thread, &format!("⚠️ split failed: {}", crate::types::mask_home(&e.to_string())), None)
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
    attach_shell_pane(s, chat, thread, &new, space, false).await;
}

/// Bare-`/split` direction from live tab geometry: the target pane's
/// longer axis wins. Any unreadable step falls back to right (the old
/// bare default) — a failed probe must never block the split.
async fn auto_split_dir(socket: &str, pane: &str) -> String {
    let tab = pane_facts(socket)
        .await
        .ok()
        .and_then(|m| m.get(pane).map(|f| f.tab_id.clone()))
        .filter(|t| !t.is_empty());
    let Some(tab) = tab else {
        return "right".to_string();
    };
    match pane_layout(socket, &tab).await.ok().and_then(|m| m.get(pane).copied()) {
        Some((w, h)) => best_split_direction(w, h).to_string(),
        None => "right".to_string(),
    }
}
