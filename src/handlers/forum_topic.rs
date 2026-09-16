//! Agent-topic message routing: commands + bare-message prompts.
//! Split from `forum` (300-line file limit).
use crate::{
    herdr::client::{get_agent, read_agent_output, send_agent_keys},
    jobs::enqueue_prompt,
    state::AppState,
    ui::{agent_card_kb, build_agent_card_text, topic_help_text},
};

use super::forum::bare_cmd;

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
        // No agent in this pane: shell CLI mode (or a lingering dead pane,
        // which the shell side reports as gone).
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

    if cmd == "/help" {
        s.tg.send_msg(
            chat,
            Some(thread_id),
            &topic_help_text(pane, &agent.kind),
            None,
        )
        .await;
        return;
    }

    if cmd == "/reset" {
        let _ = super::reset::run_single_topic_reset(&s, chat, Some(thread_id), pane).await;
        return;
    }

    if cmd == "/cancel" {
        s.keywait.lock().await.remove(&(chat, Some(thread_id)));
        s.runwait.lock().await.remove(&(chat, Some(thread_id)));
        s.typewait.lock().await.remove(&(chat, Some(thread_id)));
        let n = s.cancel_jobs_for(pane).await;
        let msg = if n {
            "cancelled pane job(s)"
        } else {
            "nothing running"
        };
        s.tg.send_msg(chat, Some(thread_id), msg, None).await;
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
    // /cancel (checked above): the next message belongs to the waiting
    // prompt. Intentional parity with DM/General — a literal "/kill" can
    // itself be the answer a dialog is waiting for. A race
    // lost to a resume falls through to prompt routing below.
    if let Some(wpane) = s.typewait.lock().await.remove(&(chat, Some(thread_id))) {
        match super::tap::type_text(&s, &wpane, text).await {
            Ok(()) => {
                s.tg.send_msg(
                    chat,
                    Some(thread_id),
                    &format!("⌨️ typed into {wpane} + ⏎"),
                    None,
                )
                .await;
                return;
            }
            Err(super::tap::TypeError::Resumed) => {}
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ type failed: {e}"), None)
                    .await;
                return;
            }
        }
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
            "" | "right" => "right",
            "down" => "down",
            _ => {
                s.tg.send_msg(chat, Some(thread_id), "usage: `/split [right|down]`", None)
                    .await;
                return;
            }
        };
        super::shell::open_split(&s, chat, Some(thread_id), pane, dir).await;
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
        let lines = arg.parse::<u32>().map(|n| n.clamp(1, 400)).unwrap_or(80);
        match read_agent_output(&s.cfg.socket, pane, lines).await {
            Ok(out) => {
                let body = if out.is_empty() {
                    "(no output)".into()
                } else {
                    out
                };
                s.tg.send_msg(chat, Some(thread_id), &body, None).await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None)
                    .await;
            }
        }
        return;
    }

    if cmd == "/keys" {
        if arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), "usage: `/keys y enter`", None)
                .await;
            return;
        }
        let keys: Vec<&str> = arg.split_whitespace().collect();
        // Never interleave with an owned key sequence: a tap answer
        // (blockop) or model switch (modelop) in flight owns the pane's
        // input until it lands.
        if s.blockop.lock().await.contains(pane) || s.modelop.lock().await.contains(pane) {
            s.tg.send_msg(
                chat,
                Some(thread_id),
                "tap/model op in flight — wait a beat",
                None,
            )
            .await;
            return;
        }
        match send_agent_keys(&s.cfg.socket, pane, &keys).await {
            Ok(_) => {
                s.tg.send_msg(chat, Some(thread_id), "⌨️ keys sent", None)
                    .await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None)
                    .await;
            }
        }
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
            Err(_) => {
                super::dialog::send_blocked_card(&s, chat, Some(thread_id), pane).await;
            }
        }
        return;
    }
    // No pre-focus: enqueue sets focus after a live deliver; a failed
    // submit must not pin focus on the corpse.
    enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
}
