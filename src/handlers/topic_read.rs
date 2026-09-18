//! Agent-topic read commands: recent terminal output. Split from
//! `forum_topic` (300-line file limit). Pure relocation.
use crate::{herdr::client::read_agent_output, state::AppState};

/// `/read [n]` + `/output [n]`: recent output of this topic's agent.
pub(crate) async fn handle_read_agent(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    arg: &str,
) {
    let lines = arg.parse::<u32>().map(|n| n.clamp(1, 400)).unwrap_or(80);
    match read_agent_output(&s.cfg.socket, pane, lines).await {
        Ok(out) => {
            let body = if out.is_empty() {
                "(no output)".into()
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
