//! Tests for [`super::format_title`] (split: 300-line file limit).
use super::*;

#[test]
fn test_title_format() {
    // Bare titles on every kind: kind lives in the icon only.
    assert_eq!(format_title("tg", "o2", "opencode"), "o2 . tg");
    assert_eq!(format_title("tg", "a1", "agy"), "a1 . tg");
    assert_eq!(format_title("tg", "main", "shell"), "main . tg");
    assert_eq!(format_title("tg", "main", "opencode"), "main . tg");
    assert_eq!(format_title("tg", "main", "agy"), "main . tg");
    assert_eq!(format_title("demo", "c1", "claude"), "c1 . demo");
    assert_eq!(
        format_title("a-very-long-workspace-label-here", "o1", "opencode"),
        "o1 . a-very-long-workspac"
    );
    assert_eq!(
        format_title("shop", "shop-backend", "claude"),
        "shop-backend . shop"
    );
    // Pasted rendered titles unwrap to the same bare title.
    assert_eq!(
        format_title("shop", "[shop] shop-backend · claude", "claude"),
        "shop-backend . shop"
    );
    assert_eq!(format_title("tg", "o2 · tg", "opencode"), "o2 . tg");
    assert_eq!(format_title("space-1", "sh1", "shell"), "sh1 . space-1");
    assert_eq!(format_title("infra", "s1", "shell"), "s1 . infra");
    // Unknown kinds render bare too.
    assert_eq!(format_title("tg", "x1", "my-agent"), "x1 . tg");
    assert_eq!(format_title("tg", "x1", "?"), "x1 . tg");
    assert_eq!(
        format_title("herdr-telegram", "main · herdr-telegram dev", "opencode"),
        "main · herdr-telegram dev . herdr-telegram"
    );
    // Minimal-dedup: space head-word echoes collapse...
    assert_eq!(format_title("ip", "ip shell", "shell"), "shell . ip");
    assert_eq!(format_title("ip", "IP SHELL", "shell"), "SHELL . ip");
    assert_eq!(format_title("ip", "ip ip shell", "shell"), "shell . ip");
    assert_eq!(format_title("ip", "ip ·", "shell"), "ip . ip");
    assert_eq!(
        format_title("tg", "tg opencode", "opencode"),
        "opencode . tg"
    );
    // ...legacy suffixed labels shed and re-sync bare...
    assert_eq!(format_title("tg", "o2 · o", "opencode"), "o2 . tg");
    assert_eq!(format_title("tg", "o2 · opencode", "opencode"), "o2 . tg");
    assert_eq!(format_title("ip", "sh1 · shell", "shell"), "sh1 . ip");
    assert_eq!(format_title("ip", "foo · SHELL", "shell"), "foo . ip");
    assert_eq!(format_title("ip", "sh1 · $", "shell"), "sh1 . ip");
    assert_eq!(format_title("tg", "main · $", "opencode"), "main . tg");
    // Alt separators shed known codes; Unicode alnum never sheds.
    assert_eq!(format_title("tg", "main | q", "opencode"), "main . tg");
    assert_eq!(format_title("tg", "main  |  q", "opencode"), "main . tg");
    assert_eq!(format_title("tg", "foo | é", "opencode"), "foo | é . tg");
    // Bare `$` is part of the name (only `· $` sheds).
    assert_eq!(format_title("tg", "main$", "opencode"), "main$ . tg");
    // ...kind match is case-blind (herdr kinds are lowercase anyway)...
    assert_eq!(format_title("ip", "sh1", "SHELL"), "sh1 . ip");
    // ...short-code bodies render plain, never stuttering...
    assert_eq!(format_title("tg", "o", "opencode"), "o . tg");
    assert_eq!(format_title("ip", "sh", "shell"), "sh . ip");
    assert_eq!(format_title("tg", "opencode", "opencode"), "opencode . tg");
    // ...degenerate kind-word bodies stay (documented)...
    assert_eq!(format_title("ip", "shell", "shell"), "shell . ip");
    // ...exact-space bodies stay (stable, unambiguous)...
    assert_eq!(format_title("ip", "ip", "shell"), "ip . ip");
    // ...and compounds are untouched.
    assert_eq!(
        format_title("shop", "shop-backend", "shell"),
        "shop-backend . shop"
    );
    assert_eq!(format_title("ip", "ip: shell", "shell"), "ip: shell . ip");
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

/// The rename round-trip, in the new shape.
///
/// Renaming works by reading a topic's current title, recovering the
/// core, and re-rendering. The space moved from a leading `[space]` to a
/// trailing `. space`, so the reverse pass has to strip the NEW shape or
/// every rename would double-wrap (`o2 . tg` → `o2 . tg . tg`) and the
/// recovered label would be wrong. These are the properties renaming
/// depends on.
#[test]
fn test_rename_roundtrip_recovers_the_label() {
    for (space, label, kind) in [
        ("tg", "o2", "opencode"),
        ("herdr-telegram", "main", "opencode"),
        ("shop", "shop-backend", "claude"),
        ("ip", "shell", "shell"),
    ] {
        let title = format_title(space, label, kind);
        assert!(
            title.ends_with(&format!(" . {}", super::short_space_for(space))),
            "space must trail: {title:?}"
        );
        assert!(
            title.starts_with(label),
            "label must lead: {title:?} for {label:?}"
        );
        assert_eq!(
            super::super::core::topic_core(&title, space, kind),
            label,
            "reverse pass must recover the label from {title:?}"
        );
    }
}

/// Idempotent: re-formatting a rendered title changes nothing, or a
/// rename triggered by a label change would keep appending the space.
#[test]
fn test_format_is_idempotent() {
    for (space, label, kind) in [
        ("tg", "o2", "opencode"),
        ("herdr-telegram", "main · herdr-telegram dev", "opencode"),
        ("shop", "shop-backend", "claude"),
    ] {
        let once = format_title(space, label, kind);
        assert_eq!(
            format_title(space, &once, kind),
            once,
            "not idempotent: {once:?}"
        );
    }
}

/// A title already in the NEW shape, re-saved, stays put.
#[test]
fn test_pasted_new_shape_is_not_double_wrapped() {
    assert_eq!(format_title("tg", "o2 . tg", "opencode"), "o2 . tg");
    // Tight spacing a user may type is understood too.
    assert_eq!(format_title("tg", "o2.tg", "opencode"), "o2 . tg");
    // The legacy shape still parses, so old topics migrate on rename.
    assert_eq!(format_title("tg", "[tg] o2", "opencode"), "o2 . tg");
}

/// A label that merely ENDS in a dot, or names another space, is the
/// user's label and must not be eaten by the suffix strip.
#[test]
fn test_custom_label_ending_in_a_dot_is_kept() {
    assert_eq!(format_title("tg", "v1.2", "opencode"), "v1.2 . tg");
    // A trailing `. other-space` is the label's own trailing word.
    assert_eq!(
        format_title("tg", "notes . elsewhere", "opencode"),
        "notes . elsewhere . tg"
    );
}
