//! Tests for [`super::topic_core`] (split: 300-line file limit).
use super::topic_core;
use crate::topics::names::format_title;

#[test]
fn test_topic_core_strips_chrome() {
    // Screenshot case: pasted rendered title → bare pane core.
    // (Legacy full-name era input still maps back the same way.)
    assert_eq!(
        topic_core(
            "[herdr-telegram] main · opencode",
            "herdr-telegram",
            "opencode"
        ),
        "main"
    );
    assert_eq!(
        topic_core("[herdr-telegram] main · o", "herdr-telegram", "opencode"),
        "main"
    );
    assert_eq!(
        topic_core("[herdr-telegram] main", "herdr-telegram", "opencode"),
        "main"
    );
    // Kind flips map back to the same core (kind lives in the icon).
    assert_eq!(topic_core("[tg] main · sh", "tg", "shell"), "main");
    assert_eq!(topic_core("[tg] main · a", "tg", "agy"), "main");
    assert_eq!(topic_core("[tg] a1 · a", "tg", "agy"), "a1");
    assert_eq!(topic_core("[space-1] sh1 · sh", "space-1", "shell"), "sh1");
    // Plain names pass through untouched.
    assert_eq!(topic_core("main", "herdr-telegram", "opencode"), "main");
    assert_eq!(topic_core("  api  ", "tg", "opencode"), "api");
    // Custom [bracket] text survives as core, but pasted `· code`
    // chrome still sheds (it gains the wrap forward — 1:1).
    assert_eq!(
        topic_core("[urgent] fix", "shop", "opencode"),
        "[urgent] fix"
    );
    assert_eq!(
        topic_core("[urgent] fix · opencode", "shop", "opencode"),
        "[urgent] fix"
    );
    assert_eq!(
        topic_core("[other] x · opencode", "tg", "opencode"),
        "[other] x"
    );
    assert_eq!(
        topic_core("[urgent] fix · shell", "shop", "shell"),
        "[urgent] fix"
    );
    assert_eq!(topic_core("[v2] o2 · o", "tg", "opencode"), "[v2] o2");
    // Compounds survive; head-word echoes collapse like the forward pass.
    assert_eq!(
        topic_core("[shop] shop-backend · claude", "shop", "claude"),
        "shop-backend"
    );
    assert_eq!(topic_core("ip shell", "ip", "shell"), "shell");
    assert_eq!(topic_core("shop-backend", "shop", "shell"), "shop-backend");
    // Shells: bare + new-style titles strip to the tag.
    assert_eq!(topic_core("[space-1] sh1", "space-1", "shell"), "sh1");
    // Legacy short-code + space suffixes shed (this space's chrome).
    assert_eq!(topic_core("[tg] o2 · o", "tg", "opencode"), "o2");
    assert_eq!(topic_core("[tg] o2 · tg", "tg", "opencode"), "o2");
    // Legacy `· agent` (unknown-era) sheds once the kind is known.
    assert_eq!(topic_core("[tg] x1 · agent", "tg", "opencode"), "x1");
    // Stacked pastes shed to a fixed point.
    assert_eq!(
        topic_core("[tg] foo · opencode · opencode", "tg", "opencode"),
        "foo"
    );
    // Case-blind space match.
    assert_eq!(topic_core("[TG] api · opencode", "tg", "opencode"), "api");
    // Truncated display space (labels cap at 20 chars forward).
    assert_eq!(
        topic_core(
            "[a-very-long-workspac] o1 · opencode",
            "a-very-long-workspace-label-here",
            "opencode"
        ),
        "o1"
    );
    // Never empty: lone chrome falls back to the raw input.
    assert_eq!(topic_core("[tg]", "tg", "opencode"), "[tg]");
    assert_eq!(topic_core("   ", "tg", "opencode"), "");
    // Unicode after alt seps never panics nor sheds (codes are ASCII).
    assert_eq!(topic_core("foo | é", "tg", "opencode"), "foo | é");
    assert_eq!(topic_core("main | é", "tg", "opencode"), "main | é");
    // Bare `$` survives; `· $` sheds.
    assert_eq!(topic_core("main$", "tg", "opencode"), "main$");
    assert_eq!(topic_core("main · $", "tg", "opencode"), "main");
}

#[test]
fn test_topic_core_roundtrip() {
    // format → core recovers the plain label (house style is stable).
    for (space, label, kind) in [
        ("herdr-telegram", "main", "opencode"),
        ("tg", "o2", "opencode"),
        ("tg", "m", "agy"),
        ("tg", "main", "shell"),
        ("shop", "shop-backend", "claude"),
        ("space-1", "sh1", "shell"),
        ("ip", "sh1", "shell"),
    ] {
        let title = format_title(space, label, kind);
        assert_eq!(
            topic_core(&title, space, kind),
            label,
            "no roundtrip: {title}"
        );
    }
}
