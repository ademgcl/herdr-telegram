use super::shell_common::shell_card_text;
use super::shell_run::run_shell_cmd;
use crate::{
    herdr::client::{
        await_fresh_root, create_tab, ensure_tg_space, get_agent, list_agents, list_workspaces,
    },
    herdr::labels::pane_facts,
    jobs::enqueue_prompt,
    state::AppState,
    types::AgentRow,
    ui::{open_topic_kb, ws_label},
};

/// DM fallback: the caller names a rowless shell pane (invisible to
/// agent.list) — run it as a command instead of asking "who?".
/// Takes the pane directly (no focus re-read): callers pass their
/// snapshot so a concurrent set_focus cannot reroute shell text.
pub async fn run_shell_fallback(s: &AppState, chat: i64, pane: String, text: &str) {
    let p = pane;
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
        Err(e) => {
            // Fail-closed: only a confirmed-dead lookup reads as shell —
            // a blip (timeout/unreachable) retries instead of injecting
            // shell text into live agent work. A partial-but-Ok
            // agent.list omission must never prove shell either.
            if crate::herdr::rpc::should_retry_agent_lookup(&e.to_string()) {
                s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return;
            }
            match list_agents(&s.cfg.socket).await {
                Err(_) => {
                    s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return;
                }
                Ok(rows) => rows.into_iter().find(|r| r.pane == p),
            }
        }
    };
    match row {
        Some(row) => {
            enqueue_prompt(s.clone(), chat, None, row, text.to_string()).await;
        }
        None => {
            // Shell-ness is only provable via pane.list: a dropout
            // between the reads above still slips through without this
            // (probe_shell_live refuses visibly on gone/unreadable).
            if probe_shell_live(s, chat, None, &p).await {
                run_shell_cmd(s, chat, None, &p, text).await;
            }
        }
    }
}

/// Liveness probe before serving a rowless shell pane (single source
/// for the DM fallbacks: explicit, reply-corpse, rowless focus, zero
/// rows). Corpse refuses UNKNOWN_TARGET, unreadable list refuses
/// HERDR_UNREACHABLE — fail-closed, never write into the void. True
/// when the shell is live (caller serves the fallback).
pub async fn probe_shell_live(s: &AppState, chat: i64, thread: Option<i64>, pane: &str) -> bool {
    match crate::herdr::client::list_panes(&s.cfg.socket).await {
        Ok(l) if l.iter().any(|p| p == pane) => true,
        Ok(_) => {
            s.tg.send_msg(chat, thread, crate::ui::UNKNOWN_TARGET, None)
                .await;
            false
        }
        Err(_) => {
            s.tg.send_msg(chat, thread, crate::ui::HERDR_UNREACHABLE, None)
                .await;
            false
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
        s.tg.send_msg(chat, thread, crate::ui::SPACE_UNREADABLE, None)
            .await;
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
                s.tg.send_msg(chat, thread, &crate::ui::unknown_space(arg), None)
                    .await;
            }
        }
        return;
    }
    match pane_facts(&s.cfg.socket)
        .await
        .ok()
        .and_then(|m| m.get(pane).map(|f| f.ws.clone()))
    {
        Some(ws) if !ws.is_empty() => open_pane(s, chat, thread, &ws).await,
        _ => {
            s.tg.send_msg(chat, thread, crate::ui::SPACE_UNREADABLE, None)
                .await;
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
                s.tg.send_msg(chat, thread, &crate::ui::unknown_space(arg), None)
                    .await;
            }
        }
        return;
    }
    let ws = match s.get_focus().await {
        Some(p) => pane_facts(&s.cfg.socket)
            .await
            .ok()
            .and_then(|m| m.get(&p).map(|f| f.ws.clone()))
            .unwrap_or_default(),
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
                s.tg.send_msg(chat, thread, &crate::ui::unknown_space(w), None)
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
                s.tg.send_msg(
                    chat,
                    thread,
                    &format!("⚠️ {}", crate::types::mask_home(&e.to_string())),
                    None,
                )
                .await;
                return;
            }
        },
    };
    if let Some(pane) = reuse {
        attach_shell_pane(
            s,
            chat,
            thread,
            &pane,
            &space_label(s, &ws_id).await,
            follow,
        )
        .await;
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
    debug_assert!(!ws_id.is_empty(), "create_workspace never returns Ok-empty");
    match await_fresh_root(&s.cfg.socket, ws_id).await {
        Some(pane) => {
            attach_shell_pane(s, chat, thread, &pane, &space_label(s, ws_id).await, true).await
        }
        None => create_tab_and_attach(s, chat, thread, ws_id, true).await,
    }
}

/// Fresh tab in a known workspace id + shell badge. The id is already
/// resolved — never `resolve_ws` here (fresh ids may not list yet).
async fn create_tab_and_attach(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    ws_id: &str,
    follow: bool,
) {
    let pane = match create_tab(&s.cfg.socket, ws_id).await {
        // create_tab fail-closed (empty→Err): no dead Ok-empty arm.
        Ok(p) => p,
        Err(e) => {
            s.tg.send_msg(
                chat,
                thread,
                &format!("⚠️ {}", crate::types::mask_home(&e.to_string())),
                None,
            )
            .await;
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
pub(crate) async fn attach_shell_pane(
    s: &AppState,
    chat: i64,
    thread: Option<i64>,
    pane: &str,
    space: &str,
    follow_focus: bool,
) {
    // Normalized send (callback parity): a General panel tap carries
    // Some(1) — sends must target None, never thread 1.
    let thread = thread.filter(|t| *t != 1);
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
    // Focus follows delivery: a failed send pins routing nowhere.
    if follow_focus && mid.is_some() {
        s.set_focus(pane).await;
    }
}
