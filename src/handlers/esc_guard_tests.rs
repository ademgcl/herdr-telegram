//! Esc-gate verdict tests: blips retry, only classified death is gone.
use super::*;

#[test]
fn test_classify_blocked_vs_live() {
    assert_eq!(classify(Ok("blocked".to_string())), EscGate::Blocked);
    assert_eq!(classify(Ok("working".to_string())), EscGate::NotBlocked);
    assert_eq!(classify(Ok("idle".to_string())), EscGate::NotBlocked);
}

#[test]
fn test_classify_gone_vs_blip() {
    // Classified death → gone path (UNKNOWN_TARGET after probe).
    assert_eq!(classify(Err("agent_not_found".to_string())), EscGate::Gone);
    assert_eq!(
        classify(Err("Agent not found: w1:p1".to_string())),
        EscGate::Gone
    );
    assert_eq!(classify(Err("no such pane".to_string())), EscGate::Gone);
    // Blips → retryable, never the gone path.
    assert_eq!(classify(Err("herdr timed out".to_string())), EscGate::Unreachable);
    assert_eq!(
        classify(Err("herdr unreachable — try again".to_string())),
        EscGate::Unreachable
    );
    assert_eq!(
        classify(Err("no agent output yet".to_string())),
        EscGate::Unreachable
    );
}
