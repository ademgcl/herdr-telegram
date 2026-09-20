//! Agent-topic message routing: commands + bare-message prompts.
//! Split from `forum` (300-line file limit).
use crate::{
    herdr::client::{get_agent, list_agents},
    jobs::enqueue_prompt,
    state::AppState,
    ui::{
        scope_text::{
            READ_CAP, TOPIC_READ_DEFAULT, USAGE_HISTORY_TOPIC, USAGE_READ_TOPIC, USAGE_RESET_TOPIC,
            parse_count,
        },
        topic_help_text,
    },
};

use super::{
    forum::bare_cmd,
    forum_topic_status::{handle_model_topic, handle_status_topic},
    topic_keys::handle_topic_keys_agent,
};

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
        // confirm via the agent list. Any row for the pane means a blip
        // (refuse visibly); no row takes the shell side (which reports
        // gone panes). Fail-closed: an unreadable list is an outage,
        // never a corpse — and a claiming row is never overruled, so a
        // partial dropout can't run the prompt as shell input.
        let agent = match list_agents(&s.cfg.socket).await {
            Ok(a) => a.iter().any(|r| r.pane == pane),
            Err(_) => {
                s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return;
            }
        };
        if agent {
            s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                .await;
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
        let msg = crate::ui::cancel_ack(pane, n);
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
    // the escapes checked above (/cancel, /card, /esc) — see
    // `forum_typewait` (split for the 300-line cap).
    match super::forum_typewait::consume_typewait(&s, chat, thread_id, text).await {
        super::forum_typewait::WaitOut::Handled => return,
        super::forum_typewait::WaitOut::ResumedPrompt => {
            // Raced resume: answer text must never become control —
            // a literal "/kill" becomes a prompt (fail-closed). Re-read
            // the agent (DM parity): the pre-waiter snapshot may be a
            // stale kind; an unreadable re-read refuses, never enqueues
            // stale.
            match get_agent(&s.cfg.socket, pane).await {
                Ok(fresh) => {
                    enqueue_prompt(s, chat, Some(thread_id), fresh.into(), text.to_string()).await;
                }
                Err(_) => {
                    s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                }
            }
            return;
        }
        super::forum_typewait::WaitOut::Pass => {}
    }

    // Control plane works in-thread, after the waiters (an armed
    // waiter owns the next message).
    if cmd == "/agents" || cmd == "/spawn" {
        super::agents::handle_control(&s, chat, Some(thread_id), cmd, arg).await;
        return;
    }

    if cmd == "/reset" {
        // Own-pane-only: an arg names another pane — refuse (a typo must
        // never reset the wrong pane). Spawned: must not stall the pump.
        if !arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), USAGE_RESET_TOPIC, None)
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
                s.tg.send_msg(chat, Some(thread_id), crate::ui::USAGE_SPLIT, None)
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
                s.tg.send_msg(chat, Some(thread_id), USAGE_READ_TOPIC, None)
                    .await;
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
                s.tg.send_msg(chat, Some(thread_id), USAGE_HISTORY_TOPIC, None)
                    .await;
            }
        }
        return;
    }

    if cmd == "/keys" {
        handle_topic_keys_agent(&s, chat, thread_id, pane, arg).await;
        return;
    }

    if cmd == "/status" {
        handle_status_topic(&s, chat, thread_id, pane, &agent).await;
        return;
    }

    if cmd == "/model" {
        handle_model_topic(&s, chat, thread_id, pane, arg).await;
        return;
    }

    if cmd.starts_with('/') {
        s.tg.send_msg(chat, Some(thread_id), crate::ui::UNKNOWN_COMMAND, None)
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
                s.tg.send_msg(chat, Some(thread_id), &crate::ui::typed_ack(pane), None)
                    .await;
                // Resumed work owns no job — follow it to the final reply.
                crate::jobs::follow::follow_answer(&s, pane, chat, Some(thread_id), text).await;
            }
            Err(super::tap::TypeError::Resumed) => {
                // Resumed between snapshot and send: the text becomes a
                // regular prompt instead of stray input (mirrors DM).
                enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
            }
            Err(e) => {
                // In-flight tap owns the card; the reason always shows
                // (a takes-option refuse with only a fresh card leaves
                // the user guessing why) — then fresh buttons when the
                // card lands, text fallback when it doesn't.
                if s.block_held(pane).await {
                    s.tg.send_msg(chat, Some(thread_id), crate::ui::ANSWER_IN_FLIGHT, None)
                        .await;
                } else {
                    s.tg.send_msg(
                        chat,
                        Some(thread_id),
                        &format!(
                            "⚠️ type failed: {}",
                            crate::types::mask_home(&e.to_string())
                        ),
                        None,
                    )
                    .await;
                    if !super::dialog::send_blocked_card(&s, chat, Some(thread_id), pane).await {
                        s.tg.send_msg(chat, Some(thread_id), crate::ui::CARD_FAILED_PC, None)
                            .await;
                    }
                }
            }
        }
        return;
    }
    // No pre-focus: enqueue sets focus after a live deliver; a failed
    // submit must not pin focus on the corpse.
    enqueue_prompt(s, chat, Some(thread_id), agent.into(), text.to_string()).await;
}
