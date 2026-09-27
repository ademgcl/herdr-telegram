//! Tests for tag assignment and workspace icon colour (split from
//! `mod`: 300-line file limit).
use super::*;

#[test]
fn test_code_mapping() {
    assert_eq!(code("opencode"), "o");
    assert_eq!(code("claude"), "c");
    assert_eq!(code("codex"), "x");
    assert_eq!(code("agy"), "a");
    assert_eq!(code("pi"), "pi");
    assert_eq!(code("cursor"), "cu");
    // Unknown kinds: first two alphanumerics, lowercased.
    assert_eq!(code("my-agent_2"), "my");
    assert_eq!(code("Z"), "z");
    assert_eq!(code("???"), "?");
}

#[test]
fn test_codes_unique() {
    let kinds = [
        "pi", "claude", "codex", "gemini", "cursor", "devin", "agy", "cline", "opencode",
        "copilot", "kimi", "kiro", "droid", "amp", "grok", "hermes", "kilo", "qodercli", "qwen",
        "maki",
    ];
    let mut seen = std::collections::HashSet::new();
    for k in kinds {
        assert!(seen.insert(code(k)), "collision on {k}");
    }
}

#[test]
fn test_assign_sequence_and_gap_fill() {
    // The whole point: the tag IS herdr's pane number, so a topic
    // title and herdr's UI show the same digit. Gaps stay gaps —
    // herdr never reuses a freed number, so we do not renumber.
    assert_eq!(assign(&[], "opencode", Some("wZ:p1")), "1");
    assert_eq!(assign(&[], "opencode", Some("wG:p4")), "4");
    assert_eq!(
        assign(&[], "shell", Some("wT:p3")),
        "3",
        "kind is not in the id"
    );
    // A split needs no special case: herdr hands each pane the next
    // number in the space, so they are already distinct.
    assert_eq!(pane_number("wT:p1"), Some(1));
    assert_eq!(pane_number("wT:p2"), Some(2));
    // No numeric tail (never seen from herdr): fall back rather than
    // invent a number herdr never had.
    assert_eq!(assign(&[], "opencode", Some("wZ:main")), "1");
    // No pane: the counter fallback fills the smallest unused number.
    assert_eq!(assign(&[], "opencode", None), "1");
    let taken = ["1".to_string(), "2".to_string(), "3".to_string()];
    assert_eq!(assign(&taken, "opencode", None), "4");
    assert_eq!(
        assign(&taken, "claude", None),
        "4",
        "kinds share one sequence"
    );
    // Gaps refill.
    let gapped = ["1".to_string(), "3".to_string()];
    assert_eq!(assign(&gapped, "opencode", None), "2");
    // Legacy code tags still occupy their number, so upgrading never
    // hands a live pane a number already on screen.
    let legacy = ["o1".to_string(), "o3".to_string(), "c1".to_string()];
    assert_eq!(assign(&legacy, "kilo", None), "2");
    assert_eq!(digits_of("o2"), Some(2));
    assert_eq!(digits_of("12"), Some(12));
    assert_eq!(digits_of("shell"), None);
}

#[test]
fn test_workspace_icon_color_mapping() {
    // Deterministic: bracketed and unbracketed return identical color
    assert_eq!(workspace_icon_color("shop"), workspace_icon_color("[shop]"));
    // Color is always in allowed set
    assert!(TOPIC_ICON_COLORS.contains(&workspace_icon_color("shop")));
    assert!(TOPIC_ICON_COLORS.contains(&workspace_icon_color("")));
    // Numeric suffixes map deterministically to color indices
    assert_eq!(workspace_icon_color("ws0"), TOPIC_ICON_COLORS[0]);
    assert_eq!(workspace_icon_color("ws1"), TOPIC_ICON_COLORS[1]);
    assert_eq!(workspace_icon_color("ws2"), TOPIC_ICON_COLORS[2]);
    assert_eq!(workspace_icon_color("#3"), TOPIC_ICON_COLORS[3]);
}
