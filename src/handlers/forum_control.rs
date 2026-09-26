//! Topic control-plane commands (split from `forum_topic`: 300-line
//! file limit). True when the command was consumed: an armed waiter
//! never sees it (the caller checked waiters first).
use crate::state::AppState;

pub(crate) async fn handle_control_plane(
    s: &AppState,
    chat: i64,
    thread_id: i64,
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
    false
}
