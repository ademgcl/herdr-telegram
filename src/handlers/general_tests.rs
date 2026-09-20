//! Tests for [`super::general`] (split: 300-line file limit).
use super::super::general_typewait::{TypewaitProbe, classify_typewait_probe};

#[test]
fn test_classify_typewait_probe_shell_flip_parity() {
    // Live agent serves.
    assert_eq!(
        classify_typewait_probe(true, false, true, true),
        TypewaitProbe::Proceed
    );
    // Blip/timeout (not not-found) keeps + refuses, never consumes.
    assert_eq!(
        classify_typewait_probe(false, false, true, true),
        TypewaitProbe::Unreachable
    );
    assert_eq!(
        classify_typewait_probe(false, false, false, false),
        TypewaitProbe::Unreachable
    );
    // Agent→shell flip (not-found + still listed): evict + refuse.
    assert_eq!(
        classify_typewait_probe(false, true, true, true),
        TypewaitProbe::StaleShell
    );
    // Dead pane (not-found + unlisted): evict + degrade to routing.
    assert_eq!(
        classify_typewait_probe(false, true, true, false),
        TypewaitProbe::DeadPane
    );
    // Double outage (both reads unreadable): keep + refuse.
    assert_eq!(
        classify_typewait_probe(false, true, false, false),
        TypewaitProbe::Unreachable
    );
}
