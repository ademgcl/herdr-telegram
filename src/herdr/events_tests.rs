//! Event-stream tests (split from `events`: 500-line file limit).

use super::*;

#[test]
fn test_parse_status_event_wire_shape() {
    let ev: Value = serde_json::from_str(
            r#"{"data":{"agent":"opencode","agent_status":"working","pane_id":"wG:p2","workspace_id":"wG"},"event":"pane.agent_status_changed"}"#,
        )
        .unwrap();
    assert_eq!(parse_status_event(&ev), Some(("wG:p2", "working")));
}

/// The latency fix is only real if the nudge REACHES the loop, and a
/// burst must not stampede. One test, because `WAKE` is process-global:
/// two tests sharing it steal each other's permits and flake.
#[tokio::test]
async fn test_birth_nudge_wakes_the_loop_and_bursts_coalesce() {
    // 1. A single birth wakes a parked waiter.
    let waiter = tokio::spawn(crate::state::wait_reconcile());
    tokio::task::yield_now().await;
    crate::state::wake_reconcile();
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("birth nudge must wake the reconcile loop")
        .expect("waiter task");

    // 2. A burst costs ONE scan, and the tail is not lost: `Notify`
    // retains a permit, so a second wake is still observable.
    for _ in 0..5 {
        crate::state::wake_reconcile();
    }
    let mut wakes = 0;
    for _ in 0..3 {
        if tokio::time::timeout(Duration::from_millis(50), crate::state::wait_reconcile())
            .await
            .is_ok()
        {
            wakes += 1;
        }
    }
    assert!(
        (1..=2).contains(&wakes),
        "burst must coalesce, got {wakes} wakes"
    );
}

#[test]
fn test_lifecycle_events_are_exactly_the_birth_set() {
    // A pane spawned from the herdr CLI is only discoverable via one
    // of these; the status event alone cannot see it arrive.
    for e in [
        "pane.created",
        "pane.agent_detected",
        "tab.created",
        "workspace.created",
    ] {
        assert!(super::is_lifecycle_event(e), "must wake discovery: {e}");
    }
    for e in [
        "pane.agent_status_changed",
        "pane.closed",
        "tab.focused",
        "workspace.updated",
        "",
    ] {
        assert!(
            !super::is_lifecycle_event(e),
            "must not wake discovery: {e}"
        );
    }
}

#[test]
fn test_parse_status_event_rejects_malformed() {
    let missing: Value =
        serde_json::from_str(r#"{"data":{"pane_id":"wG:p2"},"event":"pane.agent_status_changed"}"#)
            .unwrap();
    assert_eq!(parse_status_event(&missing), None);
    let wrong_name: Value = serde_json::from_str(
        r#"{"data":{"agent_status":"idle","pane_id":"wG:p2"},"event":"pane_agent_status_changed"}"#,
    )
    .unwrap();
    // Caller matches the dotted name first; parser only reads data.
    assert_eq!(parse_status_event(&wrong_name), Some(("wG:p2", "idle")));
}
