//! Topic `/keys` sender (agent flavor) + the shared first-token guard.
//! Split from `forum_topic` (300-line file limit): the body moves
//! verbatim, only the target guard is new (a pane-shaped first token
//! was typed as keystrokes into the live agent — a write from a
//! misread, never again).
use crate::{
    herdr::client::{list_agents, send_agent_keys},
    state::AppState,
    ui::scope_text::{HERDR_RETRY, USAGE_KEYS_BARE, USAGE_KEYS_TOPIC, keys_first_blocked},
};

/// Refuse pane-shaped first tokens (fail-closed): `Some(text)` means
/// send it and stop, `None` means proceed. One `list_agents` fetch for
/// pane ids + kinds; an unreadable herdr refuses with retry (never
/// sends blind). Shared by both topic flavors.
pub(crate) async fn guard_keys_first_token(s: &AppState, arg: &str) -> Option<String> {
    let first = arg.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return None;
    }
    match list_agents(&s.cfg.socket).await {
        Ok(rows) => {
            let panes: Vec<String> = rows.iter().map(|r| r.pane.clone()).collect();
            let kinds: Vec<String> = rows.iter().map(|r| r.kind.clone()).collect();
            if keys_first_blocked(first, &panes, &kinds) {
                Some(USAGE_KEYS_TOPIC.to_string())
            } else {
                None
            }
        }
        Err(_) => Some(HERDR_RETRY.to_string()),
    }
}

/// Agent-topic `/keys` (moved verbatim from `forum_topic`): inline
/// keystrokes for this topic's pane, never interleaved with an owned
/// tap/model sequence.
pub(crate) async fn handle_topic_keys_agent(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    arg: &str,
) {
    if arg.is_empty() {
        s.tg.send_msg(chat, Some(thread_id), USAGE_KEYS_BARE, None).await;
        return;
    }
    if let Some(usage) = guard_keys_first_token(s, arg).await {
        s.tg.send_msg(chat, Some(thread_id), &usage, None).await;
        return;
    }
    let keys: Vec<&str> = arg.split_whitespace().collect();
    // Bounded like every keys arm (single source): refuse, never truncate.
    if let Err(msg) = super::shell_validate::validate_keys_len(keys.len()) {
        s.tg.send_msg(chat, Some(thread_id), &msg, None).await;
        return;
    }
    // Never interleave with an owned key sequence: a tap answer
    // (blockop) or model switch (modelop) in flight owns the pane's
    // input until it lands. Self-healing peeks: stale evicts.
    if s.block_held(pane).await || s.model_held(pane).await {
        s.tg
            .send_msg(
                chat,
                Some(thread_id),
                crate::ui::TAP_MODEL_IN_FLIGHT,
                None,
            )
            .await;
        return;
    }
    match send_agent_keys(&s.cfg.socket, pane, &keys).await {
        Ok(_) => {
            s.tg
                .send_msg(chat, Some(thread_id), crate::ui::KEYS_SENT, None)
                .await;
        }
        Err(e) => {
            s.tg
                .send_msg(chat, Some(thread_id), &format!("⚠️ {}", crate::types::mask_home(&e.to_string())), None)
                .await;
        }
    }
}
