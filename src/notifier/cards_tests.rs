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
