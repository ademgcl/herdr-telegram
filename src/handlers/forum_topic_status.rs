//! Topic status/model arms: split from `forum_topic` (300-line file limit).
use crate::{state::AppState, types::AgentDetail};

/// Shared forum-topic guard (pure, tested): DM mode (no forum),
/// foreign-chat ids (thread ids collide across chats), and stale
/// replays (thread ≠ live mapping) all drop silently. Single source
/// for the status + model arms so they can never drift.
pub(crate) fn topic_guard_allows(
    forum: Option<i64>,
    chat: i64,
    mapped: Option<i64>,
    thread_id: i64,
) -> bool {
    match forum {
        Some(f) if f == chat => mapped == Some(thread_id),
        _ => false,
    }
}

/// `/status` in-topic: re-render this pane's identity card in place.
/// Parity with `notifier/status.rs:98-122` (same builder, same pin slot,
/// same overwrite-only rule): a divergent format here flaps against the
/// watchdog every tick. Stale replays (thread ≠ live mapping) and resets
/// drop silently.
pub(crate) async fn handle_status_topic(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    agent: &AgentDetail,
) {
    // Forum-only pin slot (router guarantees this, defense-in-depth:
    // thread ids collide across chats — a foreign-chat mid must never
    // poison the global pin).
    if !topic_guard_allows(
        s.cfg.forum,
        chat,
        s.topics.all_mappings().get(pane).copied(),
        thread_id,
    ) {
        return;
    }
    let Some(forum) = s.cfg.forum else {
        return;
    };
    if crate::handlers::reset::is_resetting() {
        return;
    }
    // Stale replay drop: only the live mapping thread serves.
    if s.topics.all_mappings().get(pane).copied() != Some(thread_id) {
        return;
    }
    let spaces = crate::herdr::client::list_workspaces(&s.cfg.socket)
        .await
        .unwrap_or_default();
    // Post-RPC re-validation (TOCTOU): a reset/remint during the fetch
    // drops instead of editing/sending into the wrong generation.
    if crate::handlers::reset::is_resetting() {
        return;
    }
    if s.topics.all_mappings().get(pane).copied() != Some(thread_id) {
        return;
    }
    let space = crate::ui::ws_label(&spaces, &agent.ws);
    let title_opt = if agent.title.trim().is_empty() {
        None
    } else {
        Some(agent.title.as_str())
    };
    let text = crate::ui::build_identity_card_text(
        &agent.kind,
        pane,
        space,
        &agent.status,
        title_opt,
        agent.branch.as_deref(),
    );
    // Overwrite-only (report.rs parity): repeated taps edit the pinned
    // identity card — a fresh post only when it is definitely gone, or
    // the topic accumulates a stale card per tap. Transients keep the
    // pin and retry next tick.
    let mut mid_opt = s.topics.get_pin(pane);
    if let Some(mid) = mid_opt {
        match s.tg.try_edit_msg(forum, mid, &text, None).await {
            Ok(()) => {}
            Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                mid_opt = None;
            }
            Err(_) => {}
        }
    }
    // Resend targets the validated `thread_id` (== live mapping after
    // the re-check above), never a bare re-read — a stale tap must not
    // inject into a reminted topic. CAS-store: a remint landing mid-send
    // drops the dead mid instead of clobbering the fresh pin.
    if mid_opt.is_none()
        && let Some(new_mid) = s.tg.send_msg(forum, Some(thread_id), &text, None).await
        && !s.topics.set_pin_if_thread(pane, thread_id, new_mid)
    {
        println!("[forum] pin reminted during send for {pane} — dropping stale mid");
    }
}

/// `/model` in-topic: bare shows, arg switches by filter.
pub(crate) async fn handle_model_topic(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    arg: &str,
) {
    // Shared guard with the status arm above (forum mode, live chat,
    // live mapping thread) — the two arms can never drift.
    if !topic_guard_allows(
        s.cfg.forum,
        chat,
        s.topics.all_mappings().get(pane).copied(),
        thread_id,
    ) {
        return;
    }
    if crate::handlers::reset::is_resetting() {
        return;
    }
    if arg.is_empty() {
        super::model::show_model(s, chat, Some(thread_id), pane).await;
    } else {
        let filter = super::model::search_filter(arg);
        super::model::switch_by_filter(s, chat, Some(thread_id), pane, &filter, arg).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_guard_allows_live_only() {
        // Live mapping thread in the forum: allowed.
        assert!(topic_guard_allows(Some(1), 1, Some(7), 7));
        // DM mode (no forum): never.
        assert!(!topic_guard_allows(None, 1, Some(7), 7));
        // Foreign chat (thread ids collide across chats): never.
        assert!(!topic_guard_allows(Some(1), 2, Some(7), 7));
        // Stale replay (reminted thread) and unmapped pane: never.
        assert!(!topic_guard_allows(Some(1), 1, Some(8), 7));
        assert!(!topic_guard_allows(Some(1), 1, None, 7));
    }
}
