//! Agent-topic typed-answer waiter: split from `forum_topic` (300-line cap).
use crate::state::AppState;

/// Waiter outcome: handled (caller returns), resumed-as-prompt (caller
/// enqueues text as a prompt, never control), or pass-through.
pub(crate) enum WaitOut {
    Handled,
    ResumedPrompt,
    Pass,
}

/// An armed typed-answer waiter wins over every command except the
/// escapes checked before (like /cancel, /card, /esc). Peek-first: a
/// blockop race or failed send keeps the waiter; a resume race falls
/// through — a "/" answer becomes a prompt, never control.
pub(crate) async fn consume_typewait(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    text: &str,
) -> WaitOut {
    let Some((wpane, at)) = s
        .typewait
        .lock()
        .await
        .get(&(chat, Some(thread_id)))
        .cloned()
    else {
        return WaitOut::Pass;
    };
    // Corpse bound at consume: a stale arm degrades to normal routing
    // (bare text still types into blocked panes there) instead of
    // answering a dead question.
    if crate::state::guard::claim_stale(
        at,
        std::time::Instant::now(),
        crate::state::guard::TYPEWAIT_STALE_SECS,
    ) {
        s.typewait.lock().await.remove(&(chat, Some(thread_id)));
        return WaitOut::Pass;
    }
    if s.block_held(&wpane).await {
        s.tg.send_msg(chat, Some(thread_id), crate::ui::ANSWER_IN_FLIGHT, None)
            .await;
        return WaitOut::Handled;
    }
    // Shell-flip parity with DM (`dm_typewait`): a waiter stranded
    // across an agent→shell flip must not eat the next message as typed
    // input — drop it with STALE_TYPEWAIT_SHELL instead of typing into
    // a shell. Fail-closed: ambiguous reads keep the waiter + refuse,
    // never consume on a blip.
    match crate::herdr::client::get_agent(&s.cfg.socket, &wpane).await {
        Ok(_) => {}
        Err(e) => {
            let not_found = crate::herdr::rpc::is_not_found(&e.to_string());
            if !not_found {
                s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                    .await;
                return WaitOut::Handled;
            }
            match crate::herdr::client::list_panes(&s.cfg.socket).await {
                Ok(l) if l.contains(&wpane) => {
                    s.typewait.lock().await.remove(&(chat, Some(thread_id)));
                    s.tg.send_msg(chat, Some(thread_id), crate::ui::STALE_TYPEWAIT_SHELL, None)
                        .await;
                    return WaitOut::Handled;
                }
                Ok(_) => {
                    s.typewait.lock().await.remove(&(chat, Some(thread_id)));
                    return WaitOut::Pass;
                }
                Err(_) => {
                    s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                        .await;
                    return WaitOut::Handled;
                }
            }
        }
    }
    match super::tap::type_text(s, &wpane, text).await {
        Ok(()) => {
            s.typewait.lock().await.remove(&(chat, Some(thread_id)));
            s.tg.send_msg(chat, Some(thread_id), &crate::ui::typed_ack(&wpane), None)
                .await;
            WaitOut::Handled
        }
        Err(super::tap::TypeError::Resumed) => {
            s.typewait.lock().await.remove(&(chat, Some(thread_id)));
            // Resumed between snapshot and send either way: the text
            // becomes a regular prompt, never control and never a second
            // type attempt — the caller enqueues it as a prompt. (Pass
            // here fell through to the blocked fast-path below, which
            // re-tested a stale snapshot and typed twice: a wasted RPC
            // that error-cards on a blip instead of prompting.)
            WaitOut::ResumedPrompt
        }
        Err(e) => {
            s.tg.send_msg(
                chat,
                Some(thread_id),
                &format!(
                    "⚠️ type failed: {} — retry, or /cancel to abort",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
            WaitOut::Handled
        }
    }
}
