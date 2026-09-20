pub fn topic_missing(err: &str) -> bool {
    err.contains("TOPIC_ID_INVALID")
        || err.contains("TOPIC_NOT_FOUND")
        || err.contains("THREAD_ID_INVALID")
        || err.contains("THREAD_NOT_FOUND")
        || err.contains("message thread not found")
        || err.contains("Message thread not found")
        || err.contains("topic not found")
        || err.contains("Topic not found")
        || err.contains("topic deleted")
        || err.contains("Topic deleted")
        || err.contains("TOPIC_DELETED")
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
    err.contains("TOPIC_NOT_MODIFIED") || err.contains("message is not modified")
}

/// Topic (or forum access) definitely gone: the human-deleted shapes in
/// `topic_missing` plus kick/remove/chat-gone wordings. Callers prune
/// the corpse mapping so the next tick recreates instead of probing a
/// dead thread every 60s forever. Single source for every prune site
/// (probe, reopen/close/delete) — never inline these literals.
/// Pure, tested.
pub fn topic_gone(err: &str) -> bool {
    topic_missing(err)
        || err.contains("bot was kicked")
        || err.contains("bot is not a member")
        || err.contains("chat not found")
        || err.contains("CHAT_NOT_FOUND")
}

/// Close-converged verdict (pure, tested): `closeForumTopic` on an
/// already-closed topic converges like reopen's already-open — without
/// this every 60s watchdog tick errors + retries a closed topic
/// forever. Single source for `close_forum_topic` (corpse errors still
/// propagate so callers prune by thread).
pub fn is_close_converged(err: &str) -> bool {
    let low = err.to_lowercase();
    topic_not_modified(err)
        || low.contains("not closed")
        || low.contains("already closed")
        || low.contains("not open")
        || low.contains("already open")
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
        assert!(is_close_converged("Bad Request: topic already closed"));
        assert!(is_close_converged("Bad Request: topic is not closed"));
        // Already-open converges too (reopen parity).
        assert!(is_close_converged("Bad Request: topic already open"));
        assert!(is_close_converged("Bad Request: topic is not open"));
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
        assert!(!topic_gone("Bad Request: message is not modified"));
        assert!(!topic_gone("connection reset"));
    }
}
