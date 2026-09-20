//! Agent-topic read commands: recent terminal output. Split from
//! `forum_topic` (300-line file limit). Pure relocation.
use crate::{
    herdr::client::{read_agent_output, read_agent_visible},
    state::AppState,
};

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
    // Blocked/working alternate-screen panes reject recent_unwrapped —
    // visible is the only source there (screens.rs parity with
    // read_shell_output). Confirmed death and timeouts never fall back
    // (wrong-pane output / doubled sick-herdr budget).
    let out = match read_agent_output(&s.cfg.socket, pane, lines).await {
        Ok(out) => Ok(out),
        Err(e) if crate::herdr::rpc::should_fallback_visible(&e.to_string()) => {
            read_agent_visible(&s.cfg.socket, pane, lines).await
        }
        Err(e) => Err(e),
    };
    match out {
        Ok(out) => {
            let body = if out.is_empty() {
                crate::ui::NO_OUTPUT.into()
            } else {
                out
            };
            s.tg.send_msg(chat, Some(thread_id), &body, None).await;
        }
        Err(e) => {
            s.tg.send_msg(
                chat,
                Some(thread_id),
                &format!("⚠️ {}", crate::types::mask_home(&e.to_string())),
                None,
            )
            .await;
        }
    }
}
