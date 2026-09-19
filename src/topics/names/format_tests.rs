//! Tests for [`super::format_title`] (split: 300-line file limit).
use super::*;

#[test]
fn test_title_format() {
    // Bare titles on every kind: kind lives in the icon only.
    assert_eq!(format_title("tg", "o2", "opencode"), "[tg] o2");
    assert_eq!(format_title("tg", "a1", "agy"), "[tg] a1");
    assert_eq!(format_title("tg", "main", "shell"), "[tg] main");
    assert_eq!(format_title("tg", "main", "opencode"), "[tg] main");
    assert_eq!(format_title("tg", "main", "agy"), "[tg] main");
    assert_eq!(format_title("demo", "c1", "claude"), "[demo] c1");
    assert_eq!(
        format_title("a-very-long-workspace-label-here", "o1", "opencode"),
        "[a-very-long-workspac] o1"
    );
    assert_eq!(
        format_title("shop", "shop-backend", "claude"),
        "[shop] shop-backend"
    );
    // Pasted rendered titles unwrap to the same bare title.
    assert_eq!(
        format_title("shop", "[shop] shop-backend · claude", "claude"),
        "[shop] shop-backend"
    );
    assert_eq!(format_title("tg", "o2 · tg", "opencode"), "[tg] o2");
    assert_eq!(format_title("space-1", "sh1", "shell"), "[space-1] sh1");
    assert_eq!(format_title("infra", "s1", "shell"), "[infra] s1");
    // Unknown kinds render bare too.
    assert_eq!(format_title("tg", "x1", "my-agent"), "[tg] x1");
    assert_eq!(format_title("tg", "x1", "?"), "[tg] x1");
    assert_eq!(
        format_title("herdr-telegram", "main · herdr-telegram dev", "opencode"),
        "[herdr-telegram] main · herdr-telegram dev"
    );
    // Minimal-dedup: space head-word echoes collapse...
    assert_eq!(format_title("ip", "ip shell", "shell"), "[ip] shell");
    assert_eq!(format_title("ip", "IP SHELL", "shell"), "[ip] SHELL");
    assert_eq!(format_title("ip", "ip ip shell", "shell"), "[ip] shell");
    assert_eq!(format_title("ip", "ip ·", "shell"), "[ip] ip");
    assert_eq!(
        format_title("tg", "tg opencode", "opencode"),
        "[tg] opencode"
    );
    // ...legacy suffixed labels shed and re-sync bare...
    assert_eq!(format_title("tg", "o2 · o", "opencode"), "[tg] o2");
    assert_eq!(format_title("tg", "o2 · opencode", "opencode"), "[tg] o2");
    assert_eq!(format_title("ip", "sh1 · shell", "shell"), "[ip] sh1");
    assert_eq!(format_title("ip", "foo · SHELL", "shell"), "[ip] foo");
    assert_eq!(format_title("ip", "sh1 · $", "shell"), "[ip] sh1");
    assert_eq!(format_title("tg", "main · $", "opencode"), "[tg] main");
    // Alt separators shed known codes; Unicode alnum never sheds.
    assert_eq!(format_title("tg", "main | q", "opencode"), "[tg] main");
    assert_eq!(format_title("tg", "main  |  q", "opencode"), "[tg] main");
    assert_eq!(format_title("tg", "foo | é", "opencode"), "[tg] foo | é");
    // Bare `$` is part of the name (only `· $` sheds).
    assert_eq!(format_title("tg", "main$", "opencode"), "[tg] main$");
    // ...kind match is case-blind (herdr kinds are lowercase anyway)...
    assert_eq!(format_title("ip", "sh1", "SHELL"), "[ip] sh1");
    // ...short-code bodies render plain, never stuttering...
    assert_eq!(format_title("tg", "o", "opencode"), "[tg] o");
    assert_eq!(format_title("ip", "sh", "shell"), "[ip] sh");
    assert_eq!(format_title("tg", "opencode", "opencode"), "[tg] opencode");
    // ...degenerate kind-word bodies stay (documented)...
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
