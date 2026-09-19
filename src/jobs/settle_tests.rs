//! Tests for [`super::settle`] (split: 300-line file limit).
use super::*;

#[test]
fn test_confirm_arms_then_fires_on_persistence() {
    let t0 = Instant::now();
    let mut since: SettledArm = None;
    // First settled sample only arms (recording its kind).
    assert!(!confirm_due(&mut since, "done", t0));
    assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("done"));
    // Sub-second event wakes inside one transient never commit.
    assert!(!confirm_due(
        &mut since,
        "done",
        t0 + Duration::from_millis(800)
    ));
    assert!(!confirm_due(
        &mut since,
        "done",
        t0 + Duration::from_secs(4)
    ));
    // Same-kind persistence past the gate commits and disarms.
    assert!(confirm_due(&mut since, "done", t0 + Duration::from_secs(5)));
    assert!(since.is_none());
}

#[test]
fn test_confirm_rearms_on_kind_flip() {
    let t0 = Instant::now();
    let mut since: SettledArm = None;
    assert!(!confirm_due(&mut since, "done", t0));
    // done→blocked→idle flips re-arm instead of committing.
    assert!(!confirm_due(
        &mut since,
        "blocked",
        t0 + Duration::from_secs(4)
    ));
    assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("blocked"));
    assert!(!confirm_due(
        &mut since,
        "idle",
        t0 + Duration::from_secs(8)
    ));
    assert_eq!(since.as_ref().map(|(_, k)| k.as_str()), Some("idle"));
    // Only 5s of the SAME kind commits.
    assert!(!confirm_due(
        &mut since,
        "idle",
        t0 + Duration::from_secs(12)
    ));
    assert!(confirm_due(
        &mut since,
        "idle",
        t0 + Duration::from_secs(13)
    ));
    assert!(since.is_none());
}

#[test]
fn test_recheck_clears_timer_on_kind_flip() {
    // Same settled kind: persistence claim holds.
    assert!(confirm_still_valid("done", "done"));
    assert!(confirm_still_valid("idle", "idle"));
    assert!(confirm_still_valid("blocked", "blocked"));
    // Settled-kind flips void it (done→blocked→idle must not commit
    // as one persistence); working always clears.
    assert!(!confirm_still_valid("done", "blocked"));
    assert!(!confirm_still_valid("blocked", "idle"));
    assert!(!confirm_still_valid("done", "idle"));
    assert!(!confirm_still_valid("idle", "done"));
    assert!(!confirm_still_valid("done", "working"));
}
#[test]
fn test_confirm_is_event_rate_independent() {
    // Ten rapid wakes inside a 2s transient: still no commit.
    let t0 = Instant::now();
    let mut since: SettledArm = None;
    for ms in (0..2000).step_by(200) {
        assert!(
            !confirm_due(&mut since, "idle", t0 + Duration::from_millis(ms)),
            "must not fire at {ms}ms"
        );
    }
}

#[test]
fn test_settle_commit_blocked_bypasses_gate() {
    // Blocked commits at once without arming the timer; other kinds
    // still prove 5s same-kind persistence; flips re-arm.
    let t0 = Instant::now();
    let mut since: SettledArm = None;
    assert!(settle_commit("blocked", &mut since, t0, false));
    assert!(since.is_none(), "blocked must not arm the timer");
    // A pre-armed idle does not survive a blocked commit either.
    assert!(!settle_commit("idle", &mut since, t0, false));
    assert!(settle_commit("blocked", &mut since, t0, false));
    assert!(since.is_none(), "blocked must clear a stale arm");
    assert!(!settle_commit("idle", &mut since, t0, false));
    assert!(!settle_commit(
        "idle",
        &mut since,
        t0 + Duration::from_secs(4),
        false
    ));
    assert!(settle_commit(
        "idle",
        &mut since,
        t0 + Duration::from_secs(5),
        false
    ));
    assert!(!settle_commit(
        "done",
        &mut since,
        t0 + Duration::from_secs(6),
        false
    ));
}

#[test]
fn test_settle_commit_fatal_stuck_bypasses_gate() {
    // A stuck fatal provider error commits any settled kind at once
    // (flap around the error must not loop the watcher on "working");
    // healthy runs still prove persistence.
    let t0 = Instant::now();
    let mut since: SettledArm = None;
    assert!(settle_commit("idle", &mut since, t0, true));
    assert!(since.is_none(), "fatal bypass must not arm the timer");
    assert!(settle_commit("done", &mut since, t0, true));
    assert!(!settle_commit("idle", &mut since, t0, false));
}
