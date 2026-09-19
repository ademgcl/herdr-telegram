//! Topic status/model arms: split from `forum_topic` (300-line file limit).
use crate::{state::AppState, types::AgentDetail};

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
    let Some(forum) = s.cfg.forum else {
        return;
    };
    if chat != forum {
        return;
    }
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
    if arg.is_empty() {
        super::model::show_model(s, chat, Some(thread_id), pane).await;
    } else {
        let filter = super::model::search_filter(arg);
        super::model::switch_by_filter(s, chat, Some(thread_id), pane, &filter, arg).await;
    }
}
