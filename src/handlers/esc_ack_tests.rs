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
