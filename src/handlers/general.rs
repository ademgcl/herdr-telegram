use super::forum::bare_cmd;
use crate::{state::AppState, ui::general_help_text};

use super::general_typewait::ProbeOut;

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
        s.tg.send_msg(chat, thread_id, &general_help_text(), None)
            .await;
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
        s.tg.send_msg(chat, thread_id, crate::ui::REDIRECT_TOPIC, None)
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
    if let Some((wpane, armed_at)) = s.typewait.lock().await.get(&(chat, thread_id)).cloned() {
        // Corpse bound at consume (mirrors topics): stale degrades to
        // normal routing instead of answering a dead question.
        if crate::state::guard::claim_stale(
            armed_at,
            std::time::Instant::now(),
            crate::state::guard::TYPEWAIT_STALE_SECS,
        ) {
            s.typewait.lock().await.remove(&(chat, thread_id));
        } else {
            if s.block_held(&wpane).await {
                s.tg.send_msg(chat, thread_id, crate::ui::ANSWER_IN_FLIGHT, None)
                    .await;
                return;
            }
            // Shell-flip probe (split: `general_typewait`): a stranded
            // waiter drops visibly, a dead pane degrades to normal
            // routing below (waiter already evicted — never a type
            // attempt), a blip keeps + refuses.
            let degraded = match super::general_typewait::probe_typewait(&s, chat, thread_id, &wpane)
                .await
            {
                ProbeOut::Handled => return,
                ProbeOut::Degraded => true,
                ProbeOut::Proceed => false,
            };
            // A dead pane degrades to normal routing below (waiter
            // already evicted above — never a type attempt into it).
            if !degraded {
                match super::tap::type_text(&s, &wpane, text).await {
                    Ok(()) => {
                        s.typewait.lock().await.remove(&(chat, thread_id));
                        s.tg.send_msg(chat, thread_id, &crate::ui::typed_ack(&wpane), None)
                            .await;
                        // Resumed work owns no job — follow it to the final reply.
                        crate::jobs::follow::follow_answer(&s, &wpane, chat, thread_id, text).await;
                        return;
                    }
                    Err(super::tap::TypeError::Resumed) => {
                        s.typewait.lock().await.remove(&(chat, thread_id));
                        // Raced resume (DM/forum parity): the answer
                        // becomes a prompt on the waited pane, never
                        // General control (a literal "/reset" as an answer
                        // must not wipe topics). Fail-closed: an unreadable
                        // re-read keeps the waiter (original instant, never
                        // re-stamped) + refuses instead of dropping input.
                        match crate::herdr::client::get_agent(&s.cfg.socket, &wpane).await
                        {
                            Ok(a) => {
                                crate::jobs::enqueue_prompt(
                                    s.clone(),
                                    chat,
                                    thread_id,
                                    a.into(),
                                    text.to_string(),
                                )
                                .await;
                            }
                            Err(_) => {
                                s.typewait
                                    .lock()
                                    .await
                                    .insert((chat, thread_id), (wpane.clone(), armed_at));
                                s.tg.send_msg(chat, thread_id, crate::ui::HERDR_UNREACHABLE, None)
                                    .await;
                            }
                        }
                        return;
                    }
                    Err(e) => {
                        s.tg.send_msg(
                            chat,
                            thread_id,
                            &format!(
                                "⚠️ type failed: {} — retry, or /cancel to abort",
                                crate::types::mask_home(&e.to_string())
                            ),
                            None,
                        )
                        .await;
                        return;
                    }
                }
            }
        }
    }

    // Armed waiters own the next message (DM order): a "/" answer
    // belongs to the waiting prompt, never to routing. Escapes
    // (/start, /help, /cancel, /card, /esc) precede above; everything
    // else — including /agents + /spawn — follows the waiters.
    if cmd == "/agents" {
        super::agents::show_panel(&s, chat, thread_id).await;
        return;
    }

    if cmd == "/spawn" {
        super::agents::spawn_with_arg(&s, chat, thread_id, arg).await;
        return;
    }

    if cmd == "/model" {
        s.tg.send_msg(
            chat,
            thread_id,
            crate::ui::scope_text::REDIRECT_TOPIC_SCOPED,
            None,
        )
        .await;
        return;
    }

    if cmd == "/history" {
        s.tg.send_msg(
            chat,
            thread_id,
            crate::ui::scope_text::REDIRECT_TOPIC_SCOPED,
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

    // Topic/DM-scoped commands redirect (they need a pane context —
    // same pattern as the `/model` redirect above, one arm for all).
    if matches!(
        cmd,
        "/quit" | "/kill" | "/split" | "/read" | "/output" | "/status" | "/keys"
    ) {
        s.tg.send_msg(
            chat,
            thread_id,
            crate::ui::scope_text::REDIRECT_TOPIC_SCOPED,
            None,
        )
        .await;
        return;
    }

    if cmd == "/reset" {
        let s2 = s.clone();
        let target = arg.to_string();
        if !target.is_empty() {
            super::reset::spawn_single_topic_reset(&s2, chat, thread_id, target);
        } else {
            tokio::spawn(async move { super::reset::run_paced_reset(&s2, chat, thread_id).await });
        }
        return;
    }

    if cmd.starts_with('/') {
        // General keeps the /agents nudge: unknown commands here are
        // usually pane commands run in the wrong topic.
        s.tg.send_msg(chat, thread_id, crate::ui::UNKNOWN_COMMAND_GENERAL, None)
            .await;
        return;
    }

    // Bare text in General topic
    s.tg.send_msg(chat, thread_id, crate::ui::GENERAL_HINT, None)
        .await;
}

#[cfg(test)]
#[path = "general_tests.rs"]
mod tests;
