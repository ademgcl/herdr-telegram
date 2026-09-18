pub fn topic_missing(err: &str) -> bool {
    err.contains("TOPIC_ID_INVALID")
        || err.contains("THREAD_ID_INVALID")
        || err.contains("THREAD_NOT_FOUND")
        || err.contains("message thread not found")
        || err.contains("topic deleted")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_missing_detects_invalid_topic() {
        assert!(topic_missing("Bad Request: TOPIC_ID_INVALID"));
        assert!(topic_missing("Bad Request: THREAD_ID_INVALID"));
        assert!(topic_missing("Bad Request: THREAD_NOT_FOUND"));
        assert!(topic_missing("Bad Request: message thread not found"));
        assert!(!topic_missing("Bad Request: message is not modified"));
        assert!(!topic_missing("connection reset"));
        assert!(topic_not_modified("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(!topic_not_modified("Bad Request: TOPIC_ID_INVALID"));
    }
}
