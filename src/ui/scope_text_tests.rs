//! Tests for [`super::scope_text`] (split: 300-line file limit).
use super::*;

#[test]
fn test_parse_count_bare_numeric_clamp() {
    assert_eq!(parse_count("", 80, 400), Some(80));
    assert_eq!(parse_count("   ", 60, 400), Some(60));
    assert_eq!(parse_count("200", 80, 400), Some(200));
    assert_eq!(parse_count("0", 80, 400), Some(1));
    assert_eq!(parse_count("-5", 80, 400), Some(1));
    assert_eq!(parse_count("99999", 80, 400), Some(400));
    assert_eq!(parse_count("99999999999999999999", 80, 400), None);
    assert_eq!(parse_count("5", 5, 20), Some(5));
    assert_eq!(parse_count("99", 5, 20), Some(20));
}

#[test]
fn test_parse_count_rejects_targets_and_extras() {
    // Pane-shaped or foreign text never parses (was: silently served
    // own pane or swallowed as keystrokes).
    for bad in ["w8:p1", "opencode", "200 foo", "foo 200", "12ab", "3.5"] {
        assert_eq!(parse_count(bad, 80, 400), None, "must refuse {bad}");
        assert_eq!(parse_count(bad, 5, 20), None, "must refuse {bad}");
    }
}

#[test]
fn test_keys_first_blocked_matrix() {
    let panes = vec!["w8:p1".to_string(), "w8:p3".to_string()];
    let kinds = vec!["opencode".to_string(), "opencode".to_string()];
    // Exact pane id blocks (live or dead shape).
    assert!(keys_first_blocked("w8:p1", &panes, &kinds));
    assert!(keys_first_blocked("dead:p9", &panes, &kinds));
    // Any kind blocks even when ambiguous across rows.
    assert!(keys_first_blocked("opencode", &panes, &kinds));
    assert!(!keys_first_blocked("claude", &panes, &kinds));
    // Colon-shaped always blocks (shell/dead panes have no kind row).
    assert!(keys_first_blocked("w9:p2", &panes, &kinds));
    // Ordinary first words pass — own-pane keys still send.
    assert!(!keys_first_blocked("y", &panes, &kinds));
    assert!(!keys_first_blocked("enter", &panes, &kinds));
    assert!(!keys_first_blocked("Escape", &panes, &kinds));
}
