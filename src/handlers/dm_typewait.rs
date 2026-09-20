use crate::{jobs::enqueue_prompt, state::AppState};

/// Answering a waiting prompt (set by the ⌨️ button on blocked cards).
/// Checked before routing: the next message belongs to the waiter.
/// Returns true when the message was consumed. Split from dm_prompt
/// (300-line file limit).
pub(crate) async fn handle_typewait(s: &AppState, chat: i64, text: &str) -> bool {
    // Peek first (mirrors topics): a blockop race or failed send must
    // not consume the waiter — the retry is just sending again.
    // Self-healing: a stale corpse evicts instead of bricking answers.
    let Some((wpane, armed_at)) = s.typewait.lock().await.get(&(chat, None)).cloned() else {
        return false;
    };
    // Corpse bound at consume (mirrors topics): a stale arm degrades
    // to normal routing instead of answering a dead question.
    if crate::state::guard::claim_stale(
        armed_at,
        std::time::Instant::now(),
        crate::state::guard::TYPEWAIT_STALE_SECS,
    ) {
        s.typewait.lock().await.remove(&(chat, None));
        return false;
    }
    // Shell-flip parity (shell_topic): a waiter stranded across an
    // agent→shell flip must not eat the next message as typed input —
    // drop it with STALE_TYPEWAIT_SHELL instead of typing into a shell.
    // Fail-closed (reconcile parity): get_agent-ok serves; a not-found
    // Err + live pane drops as shell; any other Err (blip/timeout)
    // keeps the waiter + retries (never consume on ambiguous read).
    match crate::herdr::client::get_agent(&s.cfg.socket, &wpane).await {
        Ok(_) => {}
        Err(e) => {
            // Herdr not-found is `agent_not_found` (underscore) — the
            // space form never matches it (waiter bricks to hygiene).
            // Single source: `herdr::rpc::is_not_found` (blips keep + retry).
            let not_found = crate::herdr::rpc::is_not_found(&e.to_string());
            if !not_found {
                s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return true;
            }
            match crate::herdr::client::list_panes(&s.cfg.socket).await {
                Ok(l) if l.contains(&wpane) => {
                    s.typewait.lock().await.remove(&(chat, None));
                    s.tg.send_msg(chat, None, crate::ui::STALE_TYPEWAIT_SHELL, None)
                        .await;
                    return true;
                }
                Ok(_) => {
                    s.typewait.lock().await.remove(&(chat, None));
                    return false;
                }
                // Double outage (agent read + pane list both unreadable):
                // fail-closed — keep the waiter, refuse visibly. Falling
                // through would attempt a write on an ambiguous read.
                Err(_) => {
                    s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return true;
                }
            }
        }
    }
    if s.block_held(&wpane).await {
        s.tg.send_msg(chat, None, crate::ui::ANSWER_IN_FLIGHT, None)
            .await;
        return true;
    }
    match super::tap::type_text(s, &wpane, text).await {
        Ok(()) => {
            s.typewait.lock().await.remove(&(chat, None));
            s.tg.send_msg(chat, None, &crate::ui::typed_ack(&wpane), None)
                .await;
            // Resumed work owns no job — follow it to the final reply.
            crate::jobs::follow::follow_answer(s, &wpane, chat, None, text).await;
            true
        }
        // Raced by a resume: the waiter is consumed — route the text to
        // the waited pane as a prompt (never re-route via reply/focus:
        // the answer belongs to wpane, and focus may point elsewhere).
        // Fail-closed: a "/" answer must become a prompt, never DM
        // control (a literal "/kill" as an answer must not kill).
        Err(super::tap::TypeError::Resumed) => {
            match crate::herdr::client::get_agent(&s.cfg.socket, &wpane).await {
                Ok(a) => {
                    s.typewait.lock().await.remove(&(chat, None));
                    enqueue_prompt(
                        crate::state::AppState::clone(s),
                        chat,
                        None,
                        a.into(),
                        text.to_string(),
                    )
                    .await;
                }
                Err(_) => {
                    // Unreadable re-read after a resume: keep the
                    // waiter with its ORIGINAL instant (never consume
                    // on ambiguous read, never re-stamp now — a fresh
                    // stamp would immortalize the waiter across a
                    // prolonged outage) so the retry re-routes.
                    s.typewait
                        .lock()
                        .await
                        .insert((chat, None), (wpane, armed_at));
                    s.tg.send_msg(chat, None, crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                }
            }
            true
        }
        Err(e) => {
            s.tg.send_msg(
                chat,
                None,
                &format!(
                    "⚠️ type failed: {} — retry, or /cancel to abort",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
            true
        }
    }
}
