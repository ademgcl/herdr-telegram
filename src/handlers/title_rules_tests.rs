//! Tests for [`super::title_rules`] (split: 300-line file limit).
use super::*;
use crate::herdr::labels::PaneFacts;

fn facts(rows: &[(&str, &str, &str)]) -> HashMap<String, PaneFacts> {
    rows.iter()
        .map(|(pane, tab, ws)| {
            (
                pane.to_string(),
                PaneFacts {
                    label: None,
                    ws: ws.to_string(),
                    tab_id: tab.to_string(),
                },
            )
        })
        .collect()
}

#[test]
fn test_tab_census_counts_split_tabs() {
    let f = facts(&[
        ("w1:p1", "t1", "a"),
        ("w1:p2", "t1", "a"),
        ("w1:p3", "t2", "a"),
    ]);
    let c = tab_census(&f);
    assert_eq!(c.get("t1"), Some(&2));
    assert_eq!(c.get("t2"), Some(&1));
    assert_eq!(c.get("t9"), None);
}

#[test]
fn test_tab_of_resolves_split_flag() {
    let f = facts(&[
        ("w1:p1", "t1", "a"),
        ("w1:p2", "t1", "a"),
        ("w1:p3", "", "a"),
    ]);
    let tabs = HashMap::from([("t1".to_string(), "console".to_string())]);
    let c = tab_census(&f);
    assert_eq!(tab_of(&f, &tabs, &c, "w1:p1"), (Some("console"), true));
    assert_eq!(tab_of(&f, &tabs, &c, "w1:p9"), (None, false));
    assert_eq!(tab_of(&f, &tabs, &c, "w1:p3"), (None, false));
}

#[test]
fn test_stored_matches_label_trims() {
    assert!(stored_matches_label(Some("My Title"), "My Title"));
    assert!(stored_matches_label(Some("My Title"), "  My Title  "));
    assert!(stored_matches_label(Some("  My Title  "), "My Title"));
    assert!(!stored_matches_label(Some("My Title"), "my title"));
    assert!(!stored_matches_label(None, "My Title"));
    assert!(!stored_matches_label(Some("[My Title]"), "My Title"));
    assert!(!stored_matches_label(Some("[tg] api · opencode"), "api"));
}

#[test]
fn test_stored_covers_label_chrome_tolerant() {
    assert!(stored_covers_label(
        Some("[tg] main · o"),
        "tg",
        "opencode",
        "main"
    ));
    assert!(stored_covers_label(Some("m"), "tg", "opencode", "m"));
    assert!(stored_covers_label(
        Some("[urgent] fix"),
        "shop",
        "opencode",
        "[urgent] fix"
    ));
    assert!(!stored_covers_label(
        Some("[tg] other · o"),
        "tg",
        "opencode",
        "main"
    ));
    assert!(!stored_covers_label(None, "tg", "opencode", "main"));
    // Stale suffix under a new kind still covers (custom preserved,
    // watchdog re-wraps bare).
    assert!(stored_covers_label(
        Some("[tg] main · o"),
        "tg",
        "shell",
        "main"
    ));
    // Case-blind + whitespace-collapsed cover.
    assert!(stored_covers_label(
        Some("[TG]  API · O"),
        "tg",
        "opencode",
        "api"
    ));
}

#[test]
fn test_reset_desired_title_raw_split_custom() {
    // Raw-stored split custom re-wraps the pane label bare.
    assert_eq!(
        reset_desired_title(
            Some("[tg] Custom"),
            "tg",
            "console o27",
            "opencode",
            true,
            Some("Custom")
        ),
        "[tg] Custom"
    );
}

#[test]
fn test_reset_desired_title_verbatim_or_formatted() {
    // 1:1 Format-B always: even a verbatim pre-reset custom gains
    // the space wrap (user text preserved, kind lives in the icon).
    assert_eq!(
        reset_desired_title(Some("My Title"), "tg", "My Title", "opencode", false, None),
        "[tg] My Title"
    );
    assert_eq!(
        reset_desired_title(
            Some("My Title"),
            "tg",
            "  My Title  ",
            "opencode",
            false,
            None
        ),
        "[tg] My Title"
    );
    // Split tab cores format — a stored title covering the pane
    // label re-wraps it bare.
    assert_eq!(
        reset_desired_title(
            Some("console o27"),
            "tg",
            "console o27",
            "opencode",
            true,
            None
        ),
        "[tg] console o27"
    );
    assert_eq!(
        reset_desired_title(
            Some("Custom Name"),
            "tg",
            "console o27",
            "opencode",
            true,
            Some("Custom Name")
        ),
        "[tg] Custom Name"
    );
    // Formatted otherwise (new/changed cores, case-only changes).
    assert_eq!(
        reset_desired_title(Some("[tg] api"), "tg", "backend", "opencode", false, None),
        "[tg] backend"
    );
    assert_eq!(
        reset_desired_title(None, "tg", "backend", "opencode", false, None),
        "[tg] backend"
    );
    assert_eq!(
        reset_desired_title(Some("My Title"), "tg", "my title", "opencode", false, None),
        "[tg] my title"
    );
}

#[test]
fn test_naming_core_none_on_tag_fallback() {
    // Tab present → core (split tabs disambiguate); missing/blank →
    // None so callers format the tag unconditionally, exactly like
    // the watchdog (which never preserves a bare tag).
    assert_eq!(
        naming_core(Some("console"), "o27", false),
        Some("console".to_string())
    );
    assert_eq!(
        naming_core(Some("console"), "o27", true),
        Some("console o27".to_string())
    );
    assert_eq!(naming_core(None, "o1", false), None);
    assert_eq!(naming_core(Some("  "), "sh1", false), None);
}

#[test]
fn test_kind_bypass_flips_only() {
    assert!(!kind_bypass(None, "shell"));
    assert!(!kind_bypass(Some("shell"), "shell"));
    assert!(kind_bypass(Some("opencode"), "shell"));
    assert!(kind_bypass(Some("shell"), "opencode"));
}

#[test]
fn test_space_rename_core_sheds_both_spaces() {
    // Plain remainder passes through.
    assert_eq!(
        space_rename_core("backend", "tg", "shop", "opencode"),
        Some("backend".to_string())
    );
    // Blank remainder = space-only (keep herdr core).
    assert_eq!(space_rename_core("   ", "tg", "shop", "opencode"), None);
    // Old-space suffix sheds via the old pass.
    assert_eq!(
        space_rename_core("main · tg", "tg", "shop", "opencode"),
        Some("main".to_string())
    );
    // Head-word echo collapses against the NEW space (1:1 roundtrip).
    assert_eq!(
        space_rename_core("shop backend", "tg", "shop", "opencode"),
        Some("backend".to_string())
    );
}

#[test]
fn test_space_label_taken_guards_duplicates() {
    use crate::types::WorkspaceInfo;
    let spaces = vec![
        WorkspaceInfo {
            id: "w1".into(),
            label: "tg".into(),
            number: 1,
        },
        WorkspaceInfo {
            id: "w2".into(),
            label: "shop".into(),
            number: 2,
        },
    ];
    assert!(space_label_taken(&spaces, "w1", "shop"));
    assert!(space_label_taken(&spaces, "w1", "  SHOP  "));
    assert!(!space_label_taken(&spaces, "w1", "fresh"));
    assert!(!space_label_taken(&spaces, "w2", "shop"));
}
