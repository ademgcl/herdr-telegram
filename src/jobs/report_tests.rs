//! Tests for cancel ownership + delivery verdicts (split: 300-line limit).
use super::*;
use crate::jobs::job::Job;

#[test]
fn test_cancel_owns_intent_current_owner_clears() {
    // The watcher the map still points at owns the intent: its retire
    // clears it.
    let job = Job::new(vec![], 1, None);
    assert!(cancel_owns_intent(Some(&job), &job, 0));
}

#[test]
fn test_cancel_owns_intent_superseded_keeps_successor() {
    // A superseding enqueue replaced the map entry: the stale watcher's
    // retire must NOT clear the successor's intent (runner::cancel_watch
    // calls this before clear_pending).
    let old = Job::new(vec![], 1, None);
    let fresh = Job::new(vec![], 2, None);
    assert!(!cancel_owns_intent(Some(&fresh), &old, 0));
    assert!(cancel_owns_intent(Some(&fresh), &fresh, 0));
}

#[test]
fn test_cancel_owns_intent_same_arc_bump_keeps_successor() {
    // Same-Arc reuse bumps the epoch in place: ptr_eq alone would clear
    // the successor's intent — the entry epoch pins the generation.
    let job = Job::new(vec![], 1, None);
    assert!(cancel_owns_intent(Some(&job), &job, 0));
    job.epoch.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert!(!cancel_owns_intent(Some(&job), &job, 0));
    assert!(cancel_owns_intent(Some(&job), &job, 1));
}

#[test]
fn test_cancel_owns_intent_vacant_clears_nothing() {
    // Map already empty (quiet retire won): no clear to issue.
    let job = Job::new(vec![], 1, None);
    assert!(!cancel_owns_intent(None, &job, 0));
}

#[test]
fn test_cancelled_text_single_source() {
    // Every cancel branch retires the silent transient, then posts this
    // fresh as NEW — pin the shared text against drift.
    assert_eq!(CANCELLED, "✋ cancelled");
}
