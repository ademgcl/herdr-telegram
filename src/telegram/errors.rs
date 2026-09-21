pub fn topic_missing(err: &str) -> bool {
    // Case-insensitive: Telegram ships both snake-case constants and
    // sentence-case descriptions ("Chat not found", bare "thread not
    // found"). A missed corpse never prunes and is retried every tick.
    let low = err.to_lowercase();
    low.contains("topic_id_invalid")
        || low.contains("topic_not_found")
        || low.contains("thread_id_invalid")
        || low.contains("thread_not_found")
        || low.contains("message thread not found")
        || low.contains("thread not found")
        || low.contains("topic not found")
        || low.contains("topic deleted")
        || low.contains("topic_deleted")
}

/// Shared fatal-permission markers: the retry-fatal, edit-gone and
/// reaction-ignored predicates below differ (single source is per
/// predicate), but these two literals are shared so a Telegram wording
/// change edits one place, never three.
pub const NO_RIGHTS: &str = "not enough rights";
pub const BOT_BLOCKED: &str = "bot was blocked";

/// Already showing that title — converged, not a failure. Callers store
/// and stay quiet instead of retry-spamming every tick.
pub fn topic_not_modified(err: &str) -> bool {
    let low = err.to_lowercase();
    low.contains("topic_not_modified") || low.contains("message is not modified")
}

/// Topic (or forum access) definitely gone: the human-deleted shapes in
/// `topic_missing` plus kick/remove/chat-gone wordings. Callers prune
/// the corpse mapping so the next tick recreates instead of probing a
/// dead thread every 60s forever. Single source for every prune site
/// (probe, reopen/close/delete) — never inline these literals.
/// Pure, tested.
pub fn topic_gone(err: &str) -> bool {
    // Case-insensitive (topic_missing parity): "Chat not found" with a
    // capital C must prune like the lowercase shape.
    let low = err.to_lowercase();
    topic_missing(err)
        || low.contains("bot was kicked")
        || low.contains("bot is not a member")
        || low.contains("chat not found")
        || low.contains("chat_not_found")
}

/// Close-converged verdict (pure, tested): `closeForumTopic` on an
/// already-closed topic converges like reopen's already-open — without
/// this every 60s watchdog tick errors + retries a closed topic
/// forever. Already-open shapes must NOT converge here: the topic is
/// still open, so the close did not take effect (reopen parity).
/// Single source for `close_forum_topic` (corpse errors still
/// propagate so callers prune by thread).
pub fn is_close_converged(err: &str) -> bool {
    let low = err.to_lowercase();
    topic_not_modified(err) || low.contains("not closed") || low.contains("already closed")
}

/// Reopen-converged verdict (close parity, split helper): `reopenForumTopic`
/// converges only on already-open shapes — converging on "already closed"
/// would mask a failed reopen as success while the topic stays closed.
pub fn is_reopen_converged(err: &str) -> bool {
    let low = err.to_lowercase();
    topic_not_modified(err) || low.contains("not open") || low.contains("already open")
}

/// True when a send error rejects the message effect (unsupported chat,
/// bad effect id) — strip `message_effect_id` and retry once.
/// Effect-shaped only: a bare "not allowed" also matches unrelated fatals
/// (rights/kicked) that must fail fast instead of burning a second send
/// that fails the same way. Single source for the strip gate.
pub fn is_effect_rejection(msg: &str) -> bool {
    let low = msg.to_lowercase();
    low.contains("effect_invalid")
        || low.contains("effect invalid")
        || low.contains("message effect")
        || low.contains("effect_id")
        || low.contains("effect not allowed")
}

/// True when an edit error means the card is definitely gone or
/// uneditable (deleted topic/thread, removed message, blocked bot) —
/// callers may post a fresh card without duplicating a live one.
/// Any other error (timeout, flood-wait exhaustion, network, rights
/// loss without a gone thread) leaves the card plausibly alive:
/// callers must keep the slot and retry the edit, never send fresh.
/// Single source for the fatal match below.
pub fn edit_gone(msg: &str) -> bool {
    // Lowercase once: Telegram ships sentence-case variants ("Message
    // can't be edited", "Bot was blocked") that exact-case matches miss
    // (retry::is_fatal_msg parity) — a miss retries a corpse edit forever
    // instead of posting fresh.
    let low = msg.to_lowercase();
    topic_gone(msg)
        || low.contains("message to edit not found")
        || low.contains("message_to_edit_not_found")
        || low.contains("message can't be edited")
        || low.contains(BOT_BLOCKED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_missing_detects_invalid_topic() {
        assert!(topic_missing("Bad Request: TOPIC_ID_INVALID"));
        assert!(topic_missing("Bad Request: THREAD_ID_INVALID"));
        assert!(topic_missing("Bad Request: THREAD_NOT_FOUND"));
        assert!(topic_missing("Bad Request: message thread not found"));
        assert!(topic_missing("Bad Request: Message thread not found"));
        assert!(topic_missing("Bad Request: topic deleted"));
        assert!(topic_missing("Bad Request: Topic deleted"));
        // Case-insensitive shapes (corpse must prune, never retry forever).
        assert!(topic_missing("Bad Request: thread not found"));
        assert!(topic_missing("Bad Request: TOPIC_DELETED"));
        assert!(!topic_missing("Bad Request: message is not modified"));
        assert!(!topic_missing("connection reset"));
        assert!(topic_not_modified("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(!topic_not_modified("Bad Request: TOPIC_ID_INVALID"));
    }

    #[test]
    fn test_is_close_converged_already_closed_only() {
        // Already-closed / not-modified converges (no watchdog retry).
        assert!(is_close_converged("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(is_close_converged("Bad Request: message is not modified"));
        assert!(is_close_converged("Bad Request: Message is not modified"));
        assert!(is_close_converged("Bad Request: topic already closed"));
        assert!(is_close_converged("Bad Request: topic is not closed"));
        // Already-open must NOT converge: the topic is still open, the
        // close did not take effect (reopen parity).
        assert!(!is_close_converged("Bad Request: topic already open"));
        assert!(!is_close_converged("Bad Request: topic is not open"));
        // Corpses still propagate so callers prune by thread; blips retry.
        assert!(!is_close_converged("Bad Request: TOPIC_ID_INVALID"));
        assert!(!is_close_converged("Bad Request: THREAD_NOT_FOUND"));
        assert!(!is_close_converged("connection reset"));
    }

    #[test]
    fn test_topic_gone_covers_kick_and_chat_gone() {
        assert!(topic_gone("Bad Request: TOPIC_ID_INVALID"));
        assert!(topic_gone("Forbidden: bot was kicked from the group"));
        assert!(topic_gone("Forbidden: bot is not a member of the chat"));
        assert!(topic_gone("Bad Request: chat not found"));
        assert!(topic_gone("Bad Request: CHAT_NOT_FOUND"));
        assert!(topic_gone("Bad Request: Chat not found"));
        assert!(!topic_gone("Bad Request: message is not modified"));
        assert!(!topic_gone("connection reset"));
    }

    #[test]
    fn test_reopen_converges_only_on_open() {
        // Already-open converges (reopen parity with close).
        assert!(is_reopen_converged("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(is_reopen_converged("Bad Request: topic already open"));
        assert!(is_reopen_converged("Bad Request: topic is not open"));
        // Already-closed must NOT converge: the topic is still closed.
        assert!(!is_reopen_converged("Bad Request: topic already closed"));
        assert!(!is_reopen_converged("Bad Request: topic is not closed"));
        // Corpses still propagate so callers prune by thread; blips retry.
        assert!(!is_reopen_converged("Bad Request: TOPIC_ID_INVALID"));
        assert!(!is_reopen_converged("connection reset"));
    }
}
