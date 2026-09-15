pub fn topic_missing(err: &str) -> bool {
    err.contains("TOPIC_ID_INVALID")
}

/// Already showing that title — converged, not a failure. Callers store
/// and stay quiet instead of retry-spamming every tick.
pub fn topic_not_modified(err: &str) -> bool {
    err.contains("TOPIC_NOT_MODIFIED")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_missing_detects_invalid_topic() {
        assert!(topic_missing("Bad Request: TOPIC_ID_INVALID"));
        assert!(!topic_missing("Bad Request: message is not modified"));
        assert!(!topic_missing("connection reset"));
        assert!(topic_not_modified("Bad Request: TOPIC_NOT_MODIFIED"));
        assert!(!topic_not_modified("Bad Request: TOPIC_ID_INVALID"));
    }
}
