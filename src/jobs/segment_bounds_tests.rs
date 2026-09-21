//! Boundary regression tests: tool-echo splits, Thought/Thinking
//! narrowness, dialog rule/option cohesion. Split from `segment_tests`
//! (300-line file limit).
use super::*;

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_tool_echo_splits_scrollback_never_merges_turns() {
    // Chrome-stripped tool echoes must also SPLIT: an old-turn line above
    // one must not merge into the final card (✻/※/⏵ parity with ✱/→/●).
    for t in [
        "✻ Cooked for 5m 36s · done",
        "※ recap: completed step 1",
        "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent",
        "✱ Grep \"guard\" in src (5 matches)",
        "~ Writing command…",
    ] {
        assert!(is_boundary(t), "must split: {t}");
        assert_eq!(
            final_block(
                &v(&[
                    "     stale answer from the last turn",
                    t,
                    "     fresh answer"
                ]),
                "q"
            ),
            v(&["     fresh answer"]),
            "old turn leaked across: {t}"
        );
    }
}

#[test]
fn test_thought_headers_split_but_prose_survives() {
    // Real TUI headers still split…
    for h in [
        "Thought · 359ms",
        "Thought: 6.8s",
        "Thought for 11s",
        "+ Thought: 6.8s",
        "▸ Thought for 11s, 1.5k tokens",
        "Thinking…",
        "Thinking...",
    ] {
        assert!(is_boundary(h), "header must split: {h}");
        assert_eq!(
            final_block(&v(&["     stale turn", h, "     fresh answer"]), "q"),
            v(&["     fresh answer"]),
            "old turn leaked across: {h}"
        );
    }
    // …but genuine prose starting with those words is content.
    for p in [
        "Thoughtful review — ship it.",
        "Thoughts on the design below.",
        "Thinking it over, I'd go with B.",
    ] {
        assert!(!is_boundary(p), "prose must not split: {p}");
        assert_eq!(final_block(&v(&[p]), "q"), v(&[p]), "prose lost: {p}");
    }
}

#[test]
fn test_dialog_rule_before_first_option_keeps_header_with_options() {
    // A box rule immediately before ❯ 1. must not cut the dialog header
    // from its option list (first-option-after-rule is the common shape).
    let rule = "─".repeat(16);
    assert!(!is_dialog_boundary(&rule, Some("❯ 1. Allow once")));
    assert!(!is_dialog_boundary(&rule, Some("❯ 2. Reject")));
}
