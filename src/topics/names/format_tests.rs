//! Tests for [`super::format_title`] (split: 300-line file limit).
use super::*;

#[test]
fn test_title_format() {
    assert_eq!(format_title("tg", "o2", "opencode"), "[tg] o2 · opencode");
    assert_eq!(format_title("ajnow", "c1", "claude"), "[ajnow] c1 · claude");
    assert_eq!(
        format_title("a-very-long-workspace-label-here", "o1", "opencode"),
        "[a-very-long-workspac] o1 · opencode"
    );
    assert_eq!(
        format_title("shop", "shop-backend", "claude"),
        "[shop] shop-backend · claude"
    );
    // Legacy bracketed titles freeze verbatim (stable: no rewrite loop).
    assert_eq!(
        format_title("shop", "[shop] shop-backend · claude", "claude"),
        "[shop] shop-backend · claude"
    );
    assert_eq!(
        format_title("tg", "o2 · tg", "opencode"),
        "[tg] o2 · opencode"
    );
    // Shells take no suffix: the sh tag says it all.
    assert_eq!(format_title("space-1", "sh1", "shell"), "[space-1] sh1");
    assert_eq!(format_title("infra", "s1", "shell"), "[infra] s1");
    // Unknown kinds keep their full name; blank stays literal.
    assert_eq!(
        format_title("tg", "x1", "my-agent"),
        "[tg] x1 · my-agent"
    );
    assert_eq!(format_title("tg", "x1", "?"), "[tg] x1 · agent");
    assert_eq!(
        format_title("herdr-telegram", "main · herdr-telegram imac", "opencode"),
        "[herdr-telegram] main · herdr-telegram imac · opencode"
    );
    // Minimal-dedup: the reported stutter collapses...
    assert_eq!(format_title("ip", "ip shell", "shell"), "[ip] shell");
    assert_eq!(format_title("ip", "IP SHELL", "shell"), "[ip] SHELL");
    assert_eq!(format_title("ip", "ip ip shell", "shell"), "[ip] shell");
    assert_eq!(format_title("ip", "ip ·", "shell"), "[ip] ip");
    assert_eq!(
        format_title("tg", "tg opencode", "opencode"),
        "[tg] opencode"
    );
    // ...legacy short-code labels shed the code and re-sync...
    assert_eq!(
        format_title("tg", "o2 · o", "opencode"),
        "[tg] o2 · opencode"
    );
    assert_eq!(
        format_title("tg", "o2 · opencode", "opencode"),
        "[tg] o2 · opencode"
    );
    assert_eq!(format_title("ip", "sh1 · shell", "shell"), "[ip] sh1");
    assert_eq!(format_title("ip", "foo · SHELL", "shell"), "[ip] foo");
    assert_eq!(format_title("ip", "sh1 · $", "shell"), "[ip] sh1");
    // ...kind match is case-blind (herdr kinds are lowercase anyway)...
    assert_eq!(format_title("ip", "sh1", "SHELL"), "[ip] sh1");
    // ...but agents keep the gate: bare bodies never newly collide...
    assert_eq!(format_title("tg", "o", "opencode"), "[tg] o · opencode");
    assert_eq!(
        format_title("tg", "opencode", "opencode"),
        "[tg] opencode · opencode"
    );
    // ...degenerate kind-word shell bodies converge (documented)...
    assert_eq!(format_title("ip", "shell", "shell"), "[ip] shell");
    // ...exact-space bodies stay (stable, unambiguous)...
    assert_eq!(format_title("ip", "ip", "shell"), "[ip] ip");
    // ...and compounds are untouched.
    assert_eq!(
        format_title("shop", "shop-backend", "shell"),
        "[shop] shop-backend"
    );
    assert_eq!(format_title("ip", "ip: shell", "shell"), "[ip] ip: shell");
}

#[test]
fn test_title_fixed_point() {
    // Re-formatting an output never changes it (watchdog converges).
    for (space, label, kind) in [
        ("ip", "ip shell", "shell"),
        ("ip", "sh1 · shell", "shell"),
        ("ip", "foo · SHELL", "shell"),
        ("ip", "shell", "shell"),
        ("tg", "o", "opencode"),
        ("tg", "o2 · opencode", "opencode"),
        ("shop", "shop-backend", "claude"),
        ("ip", "ip", "shell"),
    ] {
        let once = format_title(space, label, kind);
        assert_eq!(format_title(space, &once, kind), once, "not stable: {once}");
    }
}

#[test]
fn test_strip_leading_word() {
    assert_eq!(strip_leading_word("ip shell", "ip"), Some("shell"));
    assert_eq!(strip_leading_word("ip", "ip"), Some(""));
    assert_eq!(strip_leading_word("IP · x", "ip"), Some("x"));
    // Compounds, mismatches, and empty words never strip.
    assert_eq!(strip_leading_word("shop-backend", "shop"), None);
    assert_eq!(strip_leading_word("ipx", "ip"), None);
    assert_eq!(strip_leading_word("x shell", "ip"), None);
    assert_eq!(strip_leading_word("ip shell", ""), None);
    // Non-ASCII is char-safe (no panic, correct split).
    assert_eq!(strip_leading_word("büro fax", "büro"), Some("fax"));
    assert_eq!(strip_leading_word("bürofax", "büro"), None);
    // Multi-char lowercase folds only miss, never false-strip.
    assert_eq!(strip_leading_word("İstanbul x", "istanbul"), None);
}
