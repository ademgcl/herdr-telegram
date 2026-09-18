use super::forum::bare_cmd;
use crate::{
    herdr::client::{list_agents, list_workspaces, spawn_agent},
    state::AppState,
    ui::{build_menu_text, main_menu_kb, ws_label},
};

pub(crate) async fn handle_general_forum_message(
    s: AppState,
    chat: i64,
    thread_id: Option<i64>,
    text: &str,
) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    let cmd = bare_cmd(raw_cmd);

    if cmd == "/start" || cmd == "/help" {
        let msg = "🤖 **Herdr Telegram Bot**\n\n\
                   • `/agents` — open spaces & agents control panel\n\
                   • `/spawn <kind> [space]` — spawn a new agent & topic\n\
                    • `/shell [space]` — open a fresh shell pane & topic\n\
                    • `/pane [space]` — shell tab in this space, stays here\n\
                    • `/space [name]` — new space + shell topic\n\
                   • `/model` — inside an agent topic: model picker\n\
                   • `/card` `/esc` — inside an agent topic: fresh buttons / guarded dismiss\n\
                   • `/reset` — paced reset of all topics (re-sync from Herdr)\n\
                    • `/cancel [all|<pane>]` — abort focused job(s), all = everything\n\n\
                   💡 Each active agent has its own dedicated topic in this group! Switch to an agent's topic to chat with it directly.";
        s.tg.send_msg(chat, thread_id, msg, None).await;
        return;
    }

    if cmd == "/agents" {
        let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
        let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
        s.tg.send_msg(
            chat,
            thread_id,
            &build_menu_text(&spaces, &agents),
            Some(main_menu_kb(&spaces, &agents)),
        )
        .await;
        return;
    }

    if cmd == "/spawn" {
        let (kind, ws) = match arg.split_once(char::is_whitespace) {
            Some((k, w)) => (k, Some(w)),
            None if !arg.is_empty() => (arg, None),
            _ => {
                s.tg.send_msg(
                    chat,
                    thread_id,
                    "usage: `/spawn <kind> [space]` (e.g. `/spawn opencode space-1`)",
                    None,
                )
                .await;
                return;
            }
        };
        s.tg.send_msg(chat, thread_id, &format!("⏳ spawning {kind}…"), None)
            .await;
        // Workspace may be an id or a human label (`space-1`).
        let ws_id;
        let ws = match ws {
            Some(w) => match super::space::resolve_ws(&s, w).await {
                Some(id) => {
                    ws_id = id;
                    Some(ws_id.as_str())
                }
                None => {
                    s.tg.send_msg(chat, thread_id, &format!("⚠️ unknown space `{w}`"), None)
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
                if let Some(topic_th) = s.topics.sync_topic(&row.pane, &row.kind, space).await {
                    s.tg.send_msg(
                        chat,
                        thread_id,
                        &format!(
                            "✅ Started {} [{}] in topic #{topic_th}",
                            row.kind, row.pane
                        ),
                        None,
                    )
                    .await;
                } else {
                    s.tg.send_msg(
                        chat,
                        thread_id,
                        &format!("✅ Started {} [{}]", row.kind, row.pane),
                        None,
                    )
                    .await;
                }
            }
            Err(e) => {
                s.tg.send_msg(chat, thread_id, &format!("⚠️ spawn failed: {e}"), None)
                    .await;
            }
        }
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, thread_id));
        s.runwait.lock().await.remove(&(chat, thread_id));
        s.typewait.lock().await.remove(&(chat, thread_id));
        // Scoped (never a silent global nuke): `all`, a pane id, or focus.
        let msg = s.cancel_scoped(arg).await;
        s.tg.send_msg(chat, thread_id, &msg, None).await;
        return;
    }
    // Never-stuck escapes precede ALL waiters (run/key/type): an armed
    // waiter must never eat /card or /esc as keys or a command.
    if cmd == "/card" || cmd == "/esc" {
        s.tg.send_msg(
            chat,
            thread_id,
            "open the agent's topic and run it there — each topic is one agent.",
            None,
        )
        .await;
        return;
    }
    if super::tap::consume_runkey(&s, chat, thread_id, text).await {
        return;
    }

    // Answering a waiting prompt armed by the ⌨️ button: the next message
    // in the same topic belongs to the waiter (typewait is keyed by
    // (chat, thread)). Checked before commands so the answer can't be
    // eaten by General's fallback and leak onto a later unrelated message.
    // A race lost to a resume falls through to normal routing below.
    // Peek first (mirrors topics): failures keep the waiter for retry.
    // Self-healing: a stale corpse evicts instead of bricking answers.
    if let Some(wpane) = s.typewait.lock().await.get(&(chat, thread_id)).cloned() {
        if s.block_held(&wpane).await {
            s.tg.send_msg(
                chat,
                thread_id,
                "answer already in flight — wait a beat",
                None,
            )
            .await;
            return;
        }
        match super::tap::type_text(&s, &wpane, text).await {
            Ok(()) => {
                s.typewait.lock().await.remove(&(chat, thread_id));
                s.tg.send_msg(chat, thread_id, &format!("⌨️ typed into {wpane} + ⏎"), None)
                    .await;
                return;
            }
            Err(super::tap::TypeError::Resumed) => {
                s.typewait.lock().await.remove(&(chat, thread_id));
            }
            Err(e) => {
                s.tg.send_msg(
                    chat,
                    thread_id,
                    &format!("⚠️ type failed: {e} — retry, or /cancel to abort"),
                    None,
                )
                .await;
                return;
            }
        }
    }

    if cmd == "/model" {
        s.tg.send_msg(
            chat,
            thread_id,
            "open an agent's topic and run `/model` there — each topic is one agent.",
            None,
        )
        .await;
        return;
    }

    if cmd == "/shell" {
        super::shell::open_shell(
            &s,
            chat,
            thread_id,
            if arg.is_empty() { None } else { Some(arg) },
        )
        .await;
        return;
    }

    if cmd == "/pane" {
        super::shell::open_pane_general(&s, chat, thread_id, arg).await;
        return;
    }

    if cmd == "/reset" || cmd == "/reset_topics" {
        let s2 = s.clone();
        let target = arg.to_string();
        if !target.is_empty() {
            tokio::spawn(async move {
                let _ = super::reset::run_single_topic_reset(&s2, chat, thread_id, &target).await;
            });
        } else {
            tokio::spawn(async move { super::reset::run_paced_reset(&s2, chat, thread_id).await });
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(
            chat,
            thread_id,
            "unknown command — see `/help` or `/agents`",
            None,
        )
        .await;
        return;
    }

    // Bare text in General topic
    s.tg.send_msg(
        chat,
        thread_id,
        "💡 To talk to an agent, please open its dedicated topic or use `/agents` to spawn one.",
        None,
    )
    .await;
}
