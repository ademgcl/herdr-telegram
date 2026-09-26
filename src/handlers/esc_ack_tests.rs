//! Tests for the stripped-card heal (split: 300-line file limit).
use super::*;

#[test]
fn test_heal_delay_beats_watchdog() {
    // The heal exists to re-render long before the ≤60s watchdog would:
    // a delay at/above the watchdog strands buttonless cards instead of
    // healing them. Compile-time pin (clippy assertions_on_constants).
    const {
        assert!(HEAL_DELAY_SECS >= 1 && HEAL_DELAY_SECS < 60);
    }
}

#[tokio::test]
async fn test_heal_stripped_spawns_without_blocking() {
    // Fire-and-forget contract: scheduling the 5s heal must return at
    // once (strip sits before slow RPC — blocking here would stall the
    // ack it was stripped for), never panic, and stay schedulable.
    let (s, _dir) = crate::state::cancel::isolated_state();
    let start = std::time::Instant::now();
    heal_stripped(&s, "t:p1");
    heal_stripped(&s, "t:p1");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(HEAL_DELAY_SECS),
        "heal schedule blocked instead of spawning"
    );
}

#[test]
fn test_esc_unchanged_pages_only_true_stayput() {
    // Escape dialog_moved ORDER (regression): the raced-dialog check
    // runs first — a moved screen must follow, never page "still
    // blocked" ahead of the follower (misreports a moved dialog); an
    // unreadable re-read follows (never page on a blip); only a true
    // stay-put pages.
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let dlg = v(&[
        "△ Permission required",
        "Allow once   Allow always   Reject",
    ]);
    let working = v(&["⠋ working…", "editing src/main.rs"]);
    assert_eq!(esc_unchanged(&dlg, &working), EscStay::FollowMoved);
    assert_eq!(esc_unchanged(&dlg, &[]), EscStay::FollowUnread);
    assert_eq!(esc_unchanged(&dlg, &dlg), EscStay::Page);
    // Turned-over non-empty screen is moved, not stay-put.
    let after = v(&["Confirm apply?", "Confirm   Cancel"]);
    assert_eq!(esc_unchanged(&dlg, &after), EscStay::FollowMoved);
}
