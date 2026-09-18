//! Agent-topic message routing: commands + bare-message prompts.
//! Split from `forum` (300-line file limit).
use crate::{
    herdr::client::{get_agent, list_agents, list_panes},
    jobs::enqueue_prompt,
    state::AppState,
    ui::{
        agent_card_kb, build_agent_card_text,
        scope_text::{READ_CAP, TOPIC_READ_DEFAULT, USAGE_HISTORY_TOPIC, USAGE_READ_TOPIC, USAGE_RESET_TOPIC, parse_count},
        topic_help_text,
    },
};

use super::{forum::bare_cmd, topic_keys::handle_topic_keys_agent};

pub(crate) async fn handle_topic_agent_message(
    s: AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    text: &str,
) {
    let (raw_cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (text, ""),
    };
    // Group clients may send `/cmd@BotName` — strip the mention suffix.
    let cmd = bare_cmd(raw_cmd);

    let Ok(agent) = get_agent(&s.cfg.socket, pane).await else {
        // One failed read must not reroute a live prompt as shell input:
        // confirm via pane+agent lists (recover's cascade). Only a
        // confirmed live agent retries visibly; everything else takes the
        // shell side (which reports gone panes).
        let live = list_panes(&s.cfg.socket).await.map(|l| l.contains(&pane.to_string())).unwrap_or(true);
        let agent = list_agents(&s.cfg.socket).await.map(|a| a.iter().any(|r| r.pane == pane)).unwrap_or(true);
        if live && agent {
            s.tg.send_msg(chat, Some(thread_id), crate::ui::scope_text::HERDR_RETRY, None).await;
            return;
        }
        super::shell_topic::handle_shell_topic(s, chat, thread_id, pane, text).await;
        return;
    };
    // Args stay out of the log (prompts can carry pasted secrets); the
    // command word logs only when it IS a command — a bare single-word
    // prompt would else leak fully, a multi-word one its first word.
    println!(
        "[forum] got agent {} cmd={} ({} chars)",
        agent.pane,
        if cmd.starts_with('/') { cmd } else { "prompt" },
        text.chars().count()
    );

    if cmd == "/help" || cmd == "/start" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            &topic_help_text(pane, &agent.kind),
            None,
        )
        .await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, Some(thread_id)));
        s.runwait.lock().await.remove(&(chat, Some(thread_id)));
        s.typewait.lock().await.remove(&(chat, Some(thread_id)));
        // Parity with General/DM: an explicit arg routes via scope
        // (all | pane id — a mismatch warns instead of cancelling the
        // wrong pane); bare cancels this topic's pane, never focus.
        if arg.split_whitespace().next().is_some() {
            let msg = s.cancel_scoped(arg).await;
            s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
            return;
        }
        let n = s.cancel_jobs_for(pane).await;
        let msg = if n {
            format!("✋ cancelled {pane}")
        } else {
            format!("nothing running for {pane}")
        };
        s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
        return;
    }

    // Never-stuck escapes precede waiters (like /cancel): a literal
    // "/esc" must dismiss, never become typed input.
    if cmd == "/card" {
        super::escape::handle_card_topic(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if cmd == "/esc" {
        super::escape::handle_esc_topic(&s, chat, Some(thread_id), pane).await;
        return;
    }
    if super::tap::consume_runkey(&s, chat, Some(thread_id), text).await {
        return;
    }

    // An armed typed-answer waiter wins over every command except
    // the escapes checked above (/cancel, /card, /esc): the next message
    // belongs to the waiting prompt — a literal "/kill" can itself be
    // the answer a dialog is waiting for. Peek first (a blockop race or
    // failed send must not consume the waiter); a resume race falls
    // through. Self-healing: a stale corpse evicts instead of bricking
    // answers.
    if let Some(wpane) = s.typewait.lock().await.get(&(chat, Some(thread_id))).map(|(p, _)| p.clone()) {
        if s.block_held(&wpane).await {
            s.tg.send_msg(chat, Some(thread_id), "answer already in flight — wait a beat", None).await;
            return;
        }
        match super::tap::type_text(&s, &wpane, text).await {
            Ok(()) => {
                s.typewait.lock().await.remove(&(chat, Some(thread_id)));
                s.tg.send_msg(
                    chat,
                    Some(thread_id),
                    &format!("⌨️ typed into {wpane} + ⏎"),
                    None,
                )
                .await;
                return;
            }
            Err(super::tap::TypeError::Resumed) => {
                s.typewait.lock().await.remove(&(chat, Some(thread_id)));
            }
            Err(e) => {
                s.tg.send_msg(
                    chat,
                    Some(thread_id),
                    &format!("⚠️ type failed: {e} — retry, or /cancel to abort"),
                    None,
                )
                .await;
                return;
            }
        }
    }

    // Control plane works in-thread (menus are chat-global): panel,
    // spawn and own-pane reset land here, after the waiters (an armed
    // waiter owns the next message — commands must not hijack answers).
    if cmd == "/agents" || cmd == "/spawn" {
        super::agents::handle_control(&s, chat, Some(thread_id), cmd, arg).await;
        return;
    }

    if cmd == "/reset" {
        // Own-pane-only: an arg names another pane — refuse (a typo must
        // never reset the wrong pane). Spawned: must not stall the pump.
        if !arg.is_empty() {
            s.tg
                .send_msg(chat, Some(thread_id), USAGE_RESET_TOPIC, None)
                .await;
            return;
        }
        super::reset::spawn_single_topic_reset(&s, chat, Some(thread_id), pane.to_string());
        return;
    }

    if cmd == "/quit" {
        super::shell::quit_to_shell(&s, chat, Some(thread_id), pane).await;
        return;
    }

    if cmd == "/kill" {
        super::kill::ask_kill(&s, chat, Some(thread_id), pane).await;
        return;
    }

    if cmd == "/split" {
        let dir = match arg {
            "" | "right" | "down" => arg,
            _ => {
                s.tg.send_msg(chat, Some(thread_id), "usage: `/split [right|down]` — bare picks the longer side", None)
                    .await;
                return;
            }
        };
        super::shell::open_split(&s, chat, Some(thread_id), pane, dir).await;
        return;
    }

    if cmd == "/pane" {
        super::shell::open_pane_here(&s, chat, Some(thread_id), pane, arg).await;
        return;
    }

    if cmd == "/shell" {
        // No arg: shell next to this agent (same workspace).
        let ws = if arg.is_empty() {
            agent.ws.as_str()
        } else {
            arg
        };
        super::shell::open_shell(&s, chat, Some(thread_id), Some(ws)).await;
        return;
    }

    // (Typewait is consumed above, before commands.)

    if cmd == "/read" || cmd == "/output" {
        // Own-pane-only with a count: pane-shaped args refuse instead of
        // parsing as a count (that misread served this pane on a typo).
        match parse_count(arg, TOPIC_READ_DEFAULT, READ_CAP) {
            Some(n) => super::topic_read::handle_read_agent(&s, chat, thread_id, pane, n).await,
            None => {
                s.tg.send_msg(chat, Some(thread_id), USAGE_READ_TOPIC, None).await;
            }
        }
        return;
    }

    if cmd == "/history" {
        // Counts only: foreign text was silently defaulting to this
        // pane's last 5 — refuse instead (validated count flows through).
        match parse_count(arg, 5, crate::state::history::HISTORY_CAP as u32) {
            Some(n) => {
                crate::state::history::send_history(&s, chat, Some(thread_id), pane, n as usize)
                    .await;
            }
            None => {
                s.tg.send_msg(chat, Some(thread_id), USAGE_HISTORY_TOPIC, None).await;
            }
        }
        return;
    }

    if cmd == "/keys" {
        handle_topic_keys_agent(&s, chat, thread_id, pane, arg).await;
        return;
    }

    if cmd == "/status" {
        let text = build_agent_card_text(&agent);
        let kb = agent_card_kb(pane, &agent.ws);
        s.tg.send_msg(chat, Some(thread_id), &text, Some(kb)).await;
        return;
    }

    if cmd == "/model" {
        if arg.is_empty() {
            super::model::show_model(&s, chat, Some(thread_id), pane).await;
        } else {
            let filter = super::model::search_filter(arg);
            super::model::switch_by_filter(&s, chat, Some(thread_id), pane, &filter, arg).await;
        }
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            "unknown topic command. Type `/help` for available commands.",
            None,
        )
        .await;
        return;
    }

    // Bare message inside agent's topic -> prompt that agent!
    // Blocked panes reject text prompts ("requires interactive input"),
    // so type straight into the waiting prompt instead — no dead job.
    // Focus follows success only (a failed type must not pin focus).
    if agent.status == "blocked" {
        match super::tap::type_text(&s, pane, text).await {
            Ok(()) => {
                s.set_focus(pane).await;
                s.tg.send_msg(
                    chat,
                    Some(thread_id),
                    &format!("⌨️ typed into {pane} + ⏎"),
                    None,
                )
                .await;
            }
            Err(super::tap::TypeError::Resumed) => {
                // Resumed between snapshot and send: the text becomes a
                // regular prompt instead of stray input (mirrors DM).
                enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
            }
            Err(e) => {
                // In-flight tap owns the card; a failed card post falls
                // back to text so the error is never silent. Self-healing
                // peek: a stale corpse evicts instead of refusing rescue.
                if s.block_held(pane).await {
                    s.tg.send_msg(chat, Some(thread_id), "answer already in flight — wait a beat", None).await;
                } else if !super::dialog::send_blocked_card(&s, chat, Some(thread_id), pane).await {
                    s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ type failed: {e} — card failed too, answer on the PC"), None).await;
                }
            }
        }
        return;
    }
    // No pre-focus: enqueue sets focus after a live deliver; a failed
    // submit must not pin focus on the corpse.
    enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
}
