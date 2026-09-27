//! Tests for [`super::cards`] reset-arm consume (split: 300-line limit).
use super::*;
use std::{collections::HashMap, time::Duration};

fn arm(now: Instant, secs: u64) -> (String, Instant) {
    ("done".to_string(), now + Duration::from_secs(secs))
}

#[test]
fn test_consume_reset_arm_takes_only_the_exact_arm() {
    let now = Instant::now();
    let mut db = HashMap::new();
    db.insert("w:p1".to_string(), arm(now, 0));
    db.insert("w:p2".to_string(), arm(now, 30));
    // Exact arm consumed.
    consume_reset_arm(&mut db, "w:p1", now);
    assert!(!db.contains_key("w:p1"));
    // Newer arm survives a stale consume.
    consume_reset_arm(&mut db, "w:p2", now);
    assert!(db.contains_key("w:p2"));
    // Unknown pane: no-op, never panics.
    consume_reset_arm(&mut db, "w:p9", now);
    assert_eq!(db.len(), 1);
}

#[test]
fn test_output_quiet_needs_two_real_identical_reads() {
    // Identical output: the agent stopped writing — commit.
    assert!(crate::notifier::cards_retry::output_quiet(
        "line a\nline b",
        "line a\nline b"
    ));
    // Fresh bytes: still streaming, keep waiting (this is what a mid-task
    // idle blip looks like — a premature post here is a partial "final").
    assert!(!crate::notifier::cards_retry::output_quiet(
        "line a",
        "line a\nmore"
    ));
    // Outage on either side is unknown, never quiet (fail-closed).
    assert!(!crate::notifier::cards_retry::output_quiet("", "line a"));
    assert!(!crate::notifier::cards_retry::output_quiet("line a", ""));
    assert!(!crate::notifier::cards_retry::output_quiet("", ""));
}
