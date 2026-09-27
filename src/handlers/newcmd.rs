//! `/new` — a command index, not a creator.
//!
//! It answers the question the surface hides: which commands work here,
//! and what each is for. Inside a topic the space, tab and split are
//! already bound, so only the agent-scoped commands are listed; in
//! General there is no binding, so only the creating ones are. That
//! split is the whole point — the same command works in a topic and
//! 404s in General, and nothing in the chat says so.
use crate::state::AppState;
use crate::ui::views::{Scope, new_index};

/// Reply with the index for the surface it was run from.
pub async fn handle_new(s: &AppState, chat: i64, thread: Option<i64>, scope: Scope) {
    s.tg.send_msg(chat, thread, &new_index(scope), None).await;
}
