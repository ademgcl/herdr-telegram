use super::target::resolve_target;
use crate::{jobs::enqueue_prompt, state::AppState, types::AgentRow};

/// Answering a waiting prompt (set by the ⌨️ button on blocked cards).
/// Checked before routing: the next message belongs to the waiter.
/// Returns true when the message was consumed.
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
                    s.typewait.lock().await.insert((chat, None), (wpane, armed_at));
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
                &format!("⚠️ type failed: {e} — retry, or /cancel to abort"),
                None,
            )
            .await;
            true
        }
    }
}

/// Bare text prompt routing: explicit `<pane> <prompt>`, else reply,
/// else focus, else sole agent; shell-pane fallback when the target is
/// rowless. Blocked panes get typed input, others enqueue a prompt job.
pub(crate) async fn handle_bare_prompt(
    s: &AppState,
    chat: i64,
    rows: &[AgentRow],
    text: &str,
    reply_pane: Option<String>,
) {
    let (head, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    // A bare pane id (or kind) with no prompt is an incomplete address,
    // never prompt text: enqueuing the literal id into focus/sole-agent
    // sends "w8:p1" to the wrong session as a prompt. Refuse with usage.
    if rest.trim().is_empty() && resolve_target(rows, Some(head)).is_some() {
        s.tg.send_msg(chat, None, "usage: `<pane> <prompt>` — name a pane and a prompt", None)
            .await;
        return;
    }
    let explicit = if rest.is_empty() || rows.len() <= 1 {
        None
    } else {
        resolve_target(rows, Some(head)).map(|r| (r, rest.to_string()))
    };
    let via_reply = reply_pane
        .as_deref()
        .and_then(|p| rows.iter().find(|r| r.pane == p))
        .cloned();
    let via_focus = s
        .get_focus()
        .await
        .and_then(|p| rows.iter().find(|r| r.pane == p))
        .cloned();

    let (row, prompt_text) = if let Some(pair) = explicit {
        pair
    } else if let Some(r) = via_reply {
        (r, text.to_string())
    } else if reply_pane.is_some() {
        // The reply names a rowless (shell) pane: run it as a command
        // instead of falling through to the focused agent (which would
        // send shell text to the wrong agent as a prompt).
        super::shell::run_shell_fallback(s, chat, reply_pane.clone(), text).await;
        return;
    } else if let Some(r) = via_focus {
        (r, text.to_string())
    } else if let Some(r) = resolve_target(rows, Some("")) {
        // Sole agent: a leading pane-id prefix ("w1:p1 fix bug") is an
        // address, not prompt text — strip it when present.
        let prompt_text = match text.split_once(char::is_whitespace) {
            Some((h, rest)) if h == r.pane && !rest.trim().is_empty() => rest.to_string(),
            _ => text.to_string(),
        };
        (r, prompt_text)
    } else {
        // Reply/focus may point at a shell pane (invisible to agent.list).
        super::shell::run_shell_fallback(s, chat, reply_pane.clone(), text).await;
        return;
    };

    // Blocked panes reject text prompts — type into the waiting prompt.
    // Focus follows success only.
    if row.status == "blocked" {
        match super::tap::type_text(s, &row.pane, &prompt_text).await {
            Ok(()) => {
                s.set_focus(&row.pane).await;
                s.tg.send_msg(chat, None, &crate::ui::typed_ack(&row.pane), None)
                    .await;
            }
            // Resumed between snapshot and send: the text becomes a
            // regular prompt instead of stray input.
            Err(super::tap::TypeError::Resumed) => {
                enqueue_prompt(
                    crate::state::AppState::clone(s),
                    chat,
                    None,
                    row,
                    prompt_text,
                )
                .await;
            }
            Err(e) => {
                // Same why-plus-card rule as topics: the reason always
                // shows, then fresh buttons (or text fallback).
                if s.block_held(&row.pane).await {
                    s.tg.send_msg(chat, None, crate::ui::ANSWER_IN_FLIGHT, None).await;
                } else {
                    s.tg.send_msg(chat, None, &format!("⚠️ type failed: {e}"), None).await;
                    if !super::dialog::send_blocked_card(s, chat, None, &row.pane).await {
                        s.tg.send_msg(chat, None, crate::ui::CARD_FAILED_PC, None).await;
                    }
                }
            }
        }
        return;
    }
    // No pre-focus: enqueue sets focus after a live deliver.
    enqueue_prompt(
        crate::state::AppState::clone(s),
        chat,
        None,
        row,
        prompt_text,
    )
    .await;
}
