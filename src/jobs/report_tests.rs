//! Tests for cancel ownership + delivery verdicts (split: 300-line limit).
use super::*;
use crate::jobs::job::Job;

#[test]
fn test_cancel_owns_intent_current_owner_clears() {
    // The watcher the map still points at owns the intent: its retire
    // clears it.
    let job = Job::new(vec![], 1, None);
    assert!(cancel_owns_intent(Some(&job), &job));
}

#[test]
fn test_cancel_owns_intent_superseded_keeps_successor() {
    // A superseding enqueue replaced the map entry: the stale watcher's
    // retire must NOT clear the successor's intent (runner::cancel_watch
    // calls this before clear_pending).
    let old = Job::new(vec![], 1, None);
    let fresh = Job::new(vec![], 2, None);
    assert!(!cancel_owns_intent(Some(&fresh), &old));
    assert!(cancel_owns_intent(Some(&fresh), &fresh));
}

#[test]
fn test_cancel_owns_intent_vacant_clears_nothing() {
    // Map already empty (quiet retire won): no clear to issue.
    let job = Job::new(vec![], 1, None);
    assert!(!cancel_owns_intent(None, &job));
}

#[test]
fn test_cancelled_text_single_source() {
    // Every cancel branch edits this in place, never posts fresh —
    // pin the shared text against drift.
    assert_eq!(CANCELLED, "✋ cancelled");
}
