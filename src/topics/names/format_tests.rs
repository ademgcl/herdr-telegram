//! Tests for [`super::format_title`] (split: 300-line file limit).
use super::*;

#[test]
fn test_title_format() {
    assert_eq!(format_title("tg", "o2", "opencode"), "[tg] o2 · o");
    assert_eq!(format_title("tg", "a1", "agy"), "[tg] a1 · a");
    assert_eq!(format_title("tg", "main", "shell"), "[tg] main · sh");
    assert_eq!(format_title("ajnow", "c1", "claude"), "[ajnow] c1 · c");
    assert_eq!(
        format_title("a-very-long-workspace-label-here", "o1", "opencode"),
        "[a-very-long-workspac] o1 · o"
    );
    assert_eq!(
        format_title("shop", "shop-backend", "claude"),
        "[shop] shop-backend · c"
    );
    // Legacy bracketed titles freeze verbatim (stable: no rewrite loop).
    assert_eq!(
        format_title("shop", "[shop] shop-backend · claude", "claude"),
        "[shop] shop-backend · claude"
    );
    assert_eq!(
        format_title("tg", "o2 · tg", "opencode"),
        "[tg] o2 · o"
    );
    // Shells carry the `sh` code like every kind (the PC shows live
    // kind; Telegram only has this suffix).
    assert_eq!(format_title("space-1", "sh1", "shell"), "[space-1] sh1 · sh");
    assert_eq!(format_title("infra", "s1", "shell"), "[infra] s1 · sh");
    // Unknown kinds keep short fallbacks; blank stays literal.
    assert_eq!(format_title("tg", "x1", "my-agent"), "[tg] x1 · my");
    assert_eq!(format_title("tg", "x1", "?"), "[tg] x1 · agent");
    assert_eq!(
        format_title("herdr-telegram", "main · herdr-telegram dev", "opencode"),
        "[herdr-telegram] main · herdr-telegram dev · o"
    );
    // Minimal-dedup: the reported stutter collapses...
    assert_eq!(format_title("ip", "ip shell", "shell"), "[ip] shell · sh");
    assert_eq!(format_title("ip", "IP SHELL", "shell"), "[ip] SHELL · sh");
    assert_eq!(format_title("ip", "ip ip shell", "shell"), "[ip] shell · sh");
    assert_eq!(format_title("ip", "ip ·", "shell"), "[ip] ip · sh");
    assert_eq!(
        format_title("tg", "tg opencode", "opencode"),
        "[tg] opencode · o"
    );
    // ...legacy full-name suffixes shed and re-sync to short...
    assert_eq!(
        format_title("tg", "o2 · o", "opencode"),
        "[tg] o2 · o"
    );
    assert_eq!(
        format_title("tg", "o2 · opencode", "opencode"),
        "[tg] o2 · o"
    );
    assert_eq!(format_title("ip", "sh1 · shell", "shell"), "[ip] sh1 · sh");
    assert_eq!(format_title("ip", "foo · SHELL", "shell"), "[ip] foo · sh");
    assert_eq!(format_title("ip", "sh1 · $", "shell"), "[ip] sh1 · sh");
    // ...kind match is case-blind (herdr kinds are lowercase anyway)...
    assert_eq!(format_title("ip", "sh1", "SHELL"), "[ip] sh1 · sh");
    // ...a core that IS the short code renders once, never stuttering...
    assert_eq!(format_title("tg", "o", "opencode"), "[tg] o");
    assert_eq!(format_title("ip", "sh", "shell"), "[ip] sh");
    assert_eq!(
        format_title("tg", "opencode", "opencode"),
        "[tg] opencode · o"
    );
    // ...degenerate kind-word bodies keep their suffix (documented)...
    assert_eq!(format_title("ip", "shell", "shell"), "[ip] shell · sh");
    // ...exact-space bodies stay (stable, unambiguous)...
    assert_eq!(format_title("ip", "ip", "shell"), "[ip] ip · sh");
    // ...and compounds are untouched.
    assert_eq!(
        format_title("shop", "shop-backend", "shell"),
        "[shop] shop-backend · sh"
    );
    assert_eq!(format_title("ip", "ip: shell", "shell"), "[ip] ip: shell · sh");
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
        ("tg", "m", "agy"),
        ("tg", "main · o", "opencode"),
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
