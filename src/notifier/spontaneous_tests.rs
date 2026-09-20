//! Tests for spontaneous pushes (split: 300-line file limit).
use super::*;

#[test]
fn test_dm_complete_per_owner() {
    // One healthy owner completes despite a blocked one — no
    // retry-spam to the healthy, no cross-settle repeats.
    assert!(dm_complete(&[2, 0], 2));
    assert!(dm_complete(&[1], 1));
    assert!(dm_complete(&[2, 2], 2));
    // Nobody whole: partials must not stamp (retry reposts full).
    assert!(!dm_complete(&[1, 1], 2));
    assert!(!dm_complete(&[1, 0], 2));
    assert!(!dm_complete(&[], 1));
    assert!(!dm_complete(&[0], 0));
    assert!(!dm_complete(&[], 0));
}

#[test]
fn test_liveness_fail_closed() {
    // Live pane posts; dead pane never re-mints (resurrection);
    // Err/empty reads post nothing (next tick retries).
    let live = vec!["w1:p1".to_string(), "w1:p2".to_string()];
    assert_eq!(liveness(Some(&live), "w1:p1"), Liveness::Allow);
    assert_eq!(liveness(Some(&live), "w9:p9"), Liveness::Dead);
    assert_eq!(liveness(None, "w1:p1"), Liveness::Ambiguous);
    assert_eq!(liveness(Some(&Vec::new()), "w1:p1"), Liveness::Ambiguous);
}
