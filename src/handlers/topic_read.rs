//! Agent-topic read commands: recent terminal output. Split from
//! `forum_topic` (300-line file limit). Pure relocation.
use crate::{herdr::client::read_agent_output, state::AppState};

/// `/read [n]` + `/output [n]`: recent output of this topic's agent.
/// Takes the parsed count (routers validate via `parse_count` — a pane
/// target must refuse, never parse as a count).
pub(crate) async fn handle_read_agent(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    lines: u32,
) {
    match read_agent_output(&s.cfg.socket, pane, lines).await {
        Ok(out) => {
            let body = if out.is_empty() {
                crate::ui::NO_OUTPUT.into()
            } else {
                out
            };
            s.tg.send_msg(chat, Some(thread_id), &body, None).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, Some(thread_id), &format!("⚠️ {e}"), None)
                .await;
        }
    }
}
