use super::forum::bare_cmd;
use crate::{state::AppState, ui::general_help_text};

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
            match super::tap::type_text(&s, &wpane, text).await {
                Ok(()) => {
                    s.typewait.lock().await.remove(&(chat, thread_id));
                    s.tg.send_msg(chat, thread_id, &crate::ui::typed_ack(&wpane), None)
                        .await;
                    return;
                }
                Err(super::tap::TypeError::Resumed) => {
                    s.typewait.lock().await.remove(&(chat, thread_id));
                    // Raced resume: answer text must never become General
                    // control (a literal "/reset" as an answer must not
                    // wipe topics). Fall through to bare-text guidance only.
                    if cmd.starts_with('/') {
                        s.tg.send_msg(
                            chat,
                            thread_id,
                            "that answer arrived after the question moved on — re-send as a fresh prompt in the agent's topic.",
                            None,
                        )
                        .await;
                        return;
                    }
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
