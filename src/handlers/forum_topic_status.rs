//! Topic status/model arms: split from `forum_topic` (300-line file limit).
use crate::{state::AppState, types::AgentDetail};

/// `/status` in-topic: re-render this pane's agent card in place.
pub(crate) async fn handle_status_topic(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    agent: &AgentDetail,
) {
    let spaces = crate::herdr::client::list_workspaces(&s.cfg.socket)
        .await
        .unwrap_or_default();
    let space = crate::ui::ws_label(&spaces, &agent.ws);
    let text = crate::ui::build_agent_card_text(agent, space);
    let kb = crate::ui::agent_card_kb(pane, &agent.ws, space);
    s.tg.send_msg(chat, Some(thread_id), &text, Some(kb)).await;
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
