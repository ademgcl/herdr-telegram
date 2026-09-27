//! Tests for [`super::split_bracket`] + [`super::space_rename_parts`]
//! (split: 300-line file limit).
use super::*;

#[test]
fn test_split_bracket() {
    assert_eq!(split_bracket("[shop] api"), Some(("shop", "api")));
    assert_eq!(split_bracket("  [shop]  api  "), Some(("shop", "api  ")));
    assert_eq!(split_bracket("[shop]"), Some(("shop", "")));
    assert_eq!(split_bracket("[] api"), Some(("", "api")));
    assert_eq!(split_bracket("api"), None);
    assert_eq!(split_bracket("[shop api"), None);
}

#[test]
fn test_space_rename_parts_detects_new_space() {
    // Bracket-only change: space rename, pane remainder kept.
    assert_eq!(
        space_rename_parts("[shop] api", "tg"),
        Some(("shop".to_string(), "api".to_string()))
    );
    // Both parts changed: still a space rename (pane handled too).
    assert_eq!(
        space_rename_parts("[shop] backend", "tg"),
        Some(("shop".to_string(), "backend".to_string()))
    );
    // Space-only (empty remainder): keep the herdr core.
    assert_eq!(
        space_rename_parts("[shop]", "tg"),
        Some(("shop".to_string(), "".to_string()))
    );
    // Same space (tolerant) is NOT a rename — pane path owns it.
    assert_eq!(space_rename_parts("[tg] api", "tg"), None);
    assert_eq!(space_rename_parts("[TG]  api", "tg"), None);
    assert_eq!(
        space_rename_parts(
            "[a-very-long-workspac] o1",
            "a-very-long-workspace-label-here"
        ),
        None
    );
    // No/empty brackets never rename the space.
    assert_eq!(space_rename_parts("api", "tg"), None);
    assert_eq!(space_rename_parts("[] api", "tg"), None);
    assert_eq!(space_rename_parts("[tg] api", "?"), None);
}

/// The new title shape puts the space in a TRAILING bracket:
/// `api [other]`. The rename path must read "space → other, pane → api".
/// Before this, `split_bracket` found the trailing bracket and returned
/// an EMPTY remainder, so renaming a topic into another space blanked the
/// pane/tab label.
#[test]
fn test_space_rename_parts_reads_the_trailing_bracket() {
    use super::space_rename_parts;
    // New shape: label first, space last.
    assert_eq!(
        space_rename_parts("api [other]", "tg"),
        Some(("other".to_string(), "api".to_string()))
    );
    // Split panes keep their marker with the label.
    assert_eq!(
        space_rename_parts("api 2 [other]", "tg"),
        Some(("other".to_string(), "api 2".to_string()))
    );
    // Legacy leading shape still works.
    assert_eq!(
        space_rename_parts("[other] api", "tg"),
        Some(("other".to_string(), "api".to_string()))
    );
    // The same space is not a rename.
    assert_eq!(space_rename_parts("api [tg]", "tg"), None);
}
