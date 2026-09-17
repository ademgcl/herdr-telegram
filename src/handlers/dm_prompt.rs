use super::target::resolve_target;
use crate::{jobs::enqueue_prompt, state::AppState, types::AgentRow};

/// Answering a waiting prompt (set by the ⌨️ button on blocked cards).
/// Checked before routing: the next message belongs to the waiter.
/// Returns true when the message was consumed.
pub(crate) async fn handle_typewait(s: &AppState, chat: i64, text: &str) -> bool {
    // Peek first (mirrors topics): a blockop race or failed send must
    // not consume the waiter — the retry is just sending again.
    let Some(wpane) = s.typewait.lock().await.get(&(chat, None)).cloned() else {
        return false;
    };
    if s.blockop.lock().await.contains(&wpane) {
        s.tg.send_msg(chat, None, "answer already in flight — wait a beat", None)
            .await;
        return true;
    }
    match super::tap::type_text(s, &wpane, text).await {
        Ok(()) => {
            s.typewait.lock().await.remove(&(chat, None));
            s.tg.send_msg(chat, None, &format!("⌨️ typed into {wpane} + ⏎"), None)
                .await;
            true
        }
        // Raced by a resume: the waiter is already consumed, so route
        // the text as a fresh prompt below instead of dropping it.
        Err(super::tap::TypeError::Resumed) => {
            s.typewait.lock().await.remove(&(chat, None));
            false
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
                s.tg.send_msg(chat, None, &format!("⌨️ typed into {} + ⏎", row.pane), None)
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
            Err(_) => {
                // A tap in flight owns the card: never double-post over it.
                if s.blockop.lock().await.contains(&row.pane) {
                    s.tg.send_msg(chat, None, "answer already in flight — wait a beat", None)
                        .await;
                } else {
                    super::dialog::send_blocked_card(s, chat, None, &row.pane).await;
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
