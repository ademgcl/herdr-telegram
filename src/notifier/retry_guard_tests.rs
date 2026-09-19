//! Tests for the retry-guard moved-on verdict (burst-guard parity:
//! time-bounded per-key dedup style — pure helper pinned by test).
use super::*;

#[test]
fn test_moved_on_collapse_and_missing() {
    // Same status (and idle↔done collapse) is not moved-on.
    assert!(!moved_on(Some("idle"), "idle"));
    assert!(!moved_on(Some("done"), "idle"));
    assert!(!moved_on(Some("idle"), "done"));
    assert!(!moved_on(Some("working"), "working"));
    assert!(!moved_on(Some("blocked"), "blocked"));
    // Transitions are moved-on; missing status fails closed.
    assert!(moved_on(Some("working"), "idle"));
    assert!(moved_on(Some("idle"), "working"));
    assert!(moved_on(Some("blocked"), "done"));
    assert!(moved_on(None, "idle"));
    assert!(moved_on(None, "blocked"));
}
