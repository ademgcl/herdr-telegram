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

#[tokio::test]
async fn test_settle_step_loop_epoch_mismatch_clears_arm() {
    // Generation-bound arm: a submit landing after the runner's
    // loop-top load but before snapshot_entry moves entry_epoch off
    // loop_epoch — the arm from the old generation must clear, never
    // back the new turn's commit. Checked before any select/RPC, so
    // this returns promptly with no herdr/Telegram.
    let (s, _dir) = crate::state::cancel::isolated_state();
    let job = crate::jobs::job::Job::new(Vec::new(), 1, None);
    // Arm under loop_epoch 0, then a submit bumps the epoch to 1.
    let mut since: SettledArm = Some((Instant::now(), "done".to_string()));
    job.epoch.store(1, std::sync::atomic::Ordering::Relaxed);
    let mut acc = Vec::new();
    let mut retry_wait = 5u64;
    let step = settle_step(
        &s,
        "w:p1",
        &job,
        "done",
        &mut acc,
        &mut retry_wait,
        &mut since,
        false,
        0, // runner still believes generation 0
    )
    .await;
    assert!(matches!(step, SettleStep::Continue));
    assert!(
        since.is_none(),
        "old-generation arm must clear, never commit the new turn"
    );
}

#[tokio::test]
async fn test_settle_step_matching_epoch_reaches_recheck_gate() {
    // Matching loop_epoch falls through to the existing stopped/epoch
    // gates (a stopped job still returns Continue with arm cleared —
    // never finalize a retired job's stale screen).
    let (s, _dir) = crate::state::cancel::isolated_state();
    let job = crate::jobs::job::Job::new(Vec::new(), 1, None);
    job.mark_stopped();
    let mut since: SettledArm = Some((Instant::now(), "done".to_string()));
    let mut acc = Vec::new();
    let mut retry_wait = 5u64;
    let step = settle_step(
        &s,
        "w:p1",
        &job,
        "done",
        &mut acc,
        &mut retry_wait,
        &mut since,
        false,
        0,
    )
    .await;
    assert!(matches!(step, SettleStep::Continue));
    assert!(since.is_none());
}

#[tokio::test]
async fn test_settle_step_zero_pending_returns_before_confirm() {
    // Watcher spawned before publish_submit: pending still 0, epochs
    // match — must return before the 750ms confirm (and never finalize
    // the pre-submit screen as the reply).
    let (s, _dir) = crate::state::cancel::isolated_state();
    let job = crate::jobs::job::Job::new(Vec::new(), 1, None);
    let mut since: SettledArm = Some((Instant::now(), "done".to_string()));
    let mut acc = Vec::new();
    let mut retry_wait = 5u64;
    let step = tokio::time::timeout(
        Duration::from_millis(100),
        settle_step(
            &s,
            "w:p1",
            &job,
            "done",
            &mut acc,
            &mut retry_wait,
            &mut since,
            false,
            0,
        ),
    )
    .await
    .expect("zero pending must return before the 750ms confirm");
    assert!(matches!(step, SettleStep::Continue));
    assert!(since.is_none(), "unpaid arm must clear, never commit later");
}

#[tokio::test]
async fn test_sleep_or_superseded_exits_on_stop_and_epoch() {
    // A stopped job (failed-submit retire: no notify, no epoch bump) must
    // not park the backoff for the full minute — the loop top retires it.
    let job = crate::jobs::job::Job::new(Vec::new(), 1, None);
    job.mark_stopped();
    tokio::time::timeout(
        Duration::from_secs(5),
        sleep_or_superseded(&job, 0, Duration::from_secs(60)),
    )
    .await
    .expect("stopped job must exit the backoff promptly");
    // A superseding epoch bump exits too.
    let job2 = crate::jobs::job::Job::new(Vec::new(), 1, None);
    job2.epoch.store(1, std::sync::atomic::Ordering::Relaxed);
    tokio::time::timeout(
        Duration::from_secs(5),
        sleep_or_superseded(&job2, 0, Duration::from_secs(60)),
    )
    .await
    .expect("superseded epoch must exit the backoff promptly");
}
