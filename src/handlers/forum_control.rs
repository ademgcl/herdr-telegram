//! Topic control-plane commands (split from `forum_topic`: 300-line
//! file limit). True when the command was consumed: an armed waiter
//! never sees it (the caller checked waiters first). Pane-lifecycle
//! arms (`/reset`, `/quit`, `/kill`, `/split`, `/pane`, `/shell`) live
//! here with the panel arms — `forum_topic` keeps routing + prompts.
use crate::{state::AppState, ui::scope_text::USAGE_RESET_TOPIC};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_control_plane(
    s: &AppState,
    chat: i64,
    thread_id: i64,
    pane: &str,
    agent_ws: &str,
    cmd: &str,
    arg: &str,
) -> bool {
    if cmd == "/agents" || cmd == "/spawn" {
        super::agents::handle_control(s, chat, Some(thread_id), cmd, arg).await;
        return true;
    }
    if cmd == "/transient" {
        super::transient::handle_transient(s, chat, Some(thread_id), arg).await;
        return true;
    }
    if cmd == "/reset" {
        // Own-pane-only: an arg names another pane — refuse (a typo must
        // never reset the wrong pane). Spawned: must not stall the pump.
        if !arg.is_empty() {
            s.tg.send_msg(chat, Some(thread_id), USAGE_RESET_TOPIC, None)
                .await;
            return true;
        }
        super::reset::spawn_single_topic_reset(s, chat, Some(thread_id), pane.to_string());
        return true;
    }
    if cmd == "/quit" {
        super::shell::quit_to_shell(s, chat, Some(thread_id), pane).await;
        return true;
    }
    if cmd == "/kill" {
        super::kill::ask_kill(s, chat, Some(thread_id), pane).await;
        return true;
    }
    if cmd == "/split" {
        let dir = match arg {
            "" | "right" | "down" => arg,
            _ => {
                s.tg.send_msg(chat, Some(thread_id), crate::ui::USAGE_SPLIT, None)
                    .await;
                return true;
            }
        };
        super::shell::open_split(s, chat, Some(thread_id), pane, dir).await;
        return true;
    }
    if cmd == "/pane" {
        super::shell::open_pane_here(s, chat, Some(thread_id), pane, arg).await;
        return true;
    }
    if cmd == "/shell" {
        // No arg: shell next to this agent (same workspace).
        let ws = if arg.is_empty() { agent_ws } else { arg };
        super::shell::open_shell(s, chat, Some(thread_id), Some(ws)).await;
        return true;
    }
    false
}
