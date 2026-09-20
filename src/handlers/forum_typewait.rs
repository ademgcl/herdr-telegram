//! Agent-topic typed-answer waiter: split from `forum_topic` (300-line cap).
use crate::state::AppState;

/// Waiter outcome: handled (caller returns), resumed-as-prompt (caller
/// enqueues text as a prompt, never control), or pass-through.
/// The resume carries the waiter identity so an unreadable re-read can
/// restore it (DM parity) instead of dropping the answer.
pub(crate) enum WaitOut {
    Handled,
    ResumedPrompt(String, std::time::Instant),
    Pass,
}

/// Resume-identity verdict (pure, tested): the just-removed map value
/// wins — it is the freshest waiter truth (a concurrent re-arm may have
/// replaced the snapshot's generation). Falls back to the snapshot only
/// when the map no longer holds the key (concurrent consume won). Single
/// source for the resume path so the caller can restore the ORIGINAL
/// instant on an unreadable re-read instead of dropping the answer.
pub(crate) fn pick_resume(
    prev: Option<(String, std::time::Instant)>,
    wpane: String,
    at: std::time::Instant,
) -> (String, std::time::Instant) {
    prev.unwrap_or((wpane, at))
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
        let mut tw = s.typewait.lock().await;
        if tw
            .get(&(chat, Some(thread_id)))
            .map(|(_, t)| *t == at)
            .unwrap_or(false)
        {
            tw.remove(&(chat, Some(thread_id)));
        }
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
            let mut tw = s.typewait.lock().await;
            if tw
                .get(&(chat, Some(thread_id)))
                .map(|(_, t)| *t == at)
                .unwrap_or(false)
            {
                tw.remove(&(chat, Some(thread_id)));
            }
            drop(tw);
            s.tg.send_silent(chat, Some(thread_id), &crate::ui::typed_ack(&wpane))
                .await;
            // Resumed work owns no job — follow it to the final reply.
            crate::jobs::follow::follow_answer(s, &wpane, chat, Some(thread_id), text).await;
            WaitOut::Handled
        }
        Err(super::tap::TypeError::Resumed) => {
            let key = (chat, Some(thread_id));
            let prev = s.typewait.lock().await.remove(&key);
            // Resumed between snapshot and send either way: the text
            // becomes a regular prompt, never control and never a second
            // type attempt — the caller enqueues it as a prompt. (Pass
            // here fell through to the blocked fast-path below, which
            // re-tested a stale snapshot and typed twice: a wasted RPC
            // that error-cards on a blip instead of prompting.)
            // Carry the waiter identity (DM parity): an unreadable
            // re-read restores it with the ORIGINAL instant instead of
            // dropping the answer.
            let (wpane, at) = pick_resume(prev, wpane, at);
            WaitOut::ResumedPrompt(wpane, at)
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

/// Serve a resumed-prompt waiter outcome (split from `forum_topic`,
/// 300-line file limit): re-read the WAITER pane (the routing snapshot
/// may point elsewhere after a re-arm) and enqueue as a prompt, or
/// restore the waiter on an unreadable read.
pub(crate) async fn serve_resumed_prompt(
    s: crate::state::AppState,
    chat: i64,
    thread_id: i64,
    text: String,
    wpane: String,
    armed_at: std::time::Instant,
) {
    // Raced resume: answer text never becomes control (DM
    // parity: re-read the WAITER pane — the routing snapshot
    // may point elsewhere after a re-arm; `pane` prompts wrong).
    // Unreadable re-read restores the ORIGINAL instant, never drops.
    match crate::herdr::client::get_agent(&s.cfg.socket, &wpane).await {
        Ok(fresh) => {
            crate::jobs::enqueue_prompt(s, chat, Some(thread_id), fresh.into(), text).await;
        }
        Err(_) => {
            // Restore-only (never overwrite): a B:type re-arm
            // landing between the consume and this re-read owns
            // the waiter now — a blind insert would clobber it
            // and mistype the next message into the dead pane.
            s.typewait
                .lock()
                .await
                .entry((chat, Some(thread_id)))
                .or_insert((wpane, armed_at));
            s.tg.send_msg(chat, Some(thread_id), crate::ui::HERDR_UNREACHABLE, None)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_resume_removed_value_wins() {
        // The just-removed map value is the freshest truth: a
        // concurrent re-arm replaced the snapshot's generation, so the
        // caller restores THAT pane+instant, never the stale snapshot.
        let at = std::time::Instant::now();
        let later = at + std::time::Duration::from_secs(1);
        let (p, t) = pick_resume(Some(("w1:p2".into(), later)), "w1:p1".into(), at);
        assert_eq!(p, "w1:p2");
        assert_eq!(t, later);
        // No map value left (concurrent consume won): the snapshot still
        // carries the answer's waiter identity instead of dropping it.
        let (p, t) = pick_resume(None, "w1:p1".into(), at);
        assert_eq!(p, "w1:p1");
        assert_eq!(t, at);
    }
}
