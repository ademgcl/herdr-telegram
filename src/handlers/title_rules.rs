//! Pure title-decision rules (no I/O): tab→core picking, verbatim
//! preservation, and the reset Step-4 choice. Split from `titles`
//! (300-line file limit).
use std::collections::HashMap;

/// Count panes per tab_id (split-tab guard): siblings sharing one tab
/// disambiguate titles with the tag. One census per pass — shared by
/// the watchdog tick and reset loops so the counts can never drift.
pub fn tab_census(
    facts: &HashMap<String, crate::herdr::labels::PaneFacts>,
) -> HashMap<&str, usize> {
    let mut tab_count: HashMap<&str, usize> = HashMap::new();
    for f in facts.values() {
        if !f.tab_id.is_empty() {
            *tab_count.entry(f.tab_id.as_str()).or_default() += 1;
        }
    }
    tab_count
}

/// Resolve a pane's tab name + split flag from facts: the single source
/// for watchdog, adopt, and reset paths.
pub fn tab_of<'a>(
    facts: &'a HashMap<String, crate::herdr::labels::PaneFacts>,
    tabs: &'a HashMap<String, String>,
    census: &HashMap<&str, usize>,
    pane: &str,
) -> (Option<&'a str>, bool) {
    let tab_id = facts
        .get(pane)
        .map(|f| f.tab_id.as_str())
        .unwrap_or_default();
    if tab_id.is_empty() {
        return (None, false);
    }
    let tab = tabs.get(tab_id).map(|t| t.as_str());
    let multi = census.get(tab_id).copied().unwrap_or(0) > 1;
    (tab, multi)
}

/// Tab-derived naming core (`Some`) or tag-fallback (`None`): the single
/// source for watchdog, adopt, and reset naming, so the three can never
/// drift again. `None` means "format the tag unconditionally" — the
/// watchdog never preserves a bare tag, and neither does reset.
pub fn naming_core(tab: Option<&str>, tag: &str, multi: bool) -> Option<String> {
    let t = tab.map(str::trim).filter(|t| !t.is_empty())?;
    Some(if multi {
        format!("{t} {tag}")
    } else {
        t.to_string()
    })
}

/// Watchdog-parity core: labeled splits use the pane label, else tab/tag.
/// Single source for watchdog, reset, and inspect so they never drift.
pub fn title_core_for(tab: Option<&str>, tag: &str, multi: bool, pane: Option<&str>) -> Option<String> {
    if multi && let Some(pl) = pane.map(str::trim).filter(|l| !l.is_empty()) {
        return Some(pl.to_string());
    }
    naming_core(tab, tag, multi)
}

/// Verbatim-preservation predicate: a stored topic title that already
/// equals the herdr tab name (trim-compared) means a Telegram native
/// rename just synced both sides — the watchdog must keep it exactly,
/// never reformat. Pure so it is unit-tested, not just eyeballed.
pub fn stored_matches_label(stored: Option<&str>, label: &str) -> bool {
    stored.map(str::trim) == Some(label.trim())
}

/// Chrome-tolerant keep predicate for split panes (stored raw vs herdr
/// pane label): stored is the last-visible Telegram title, so shed
/// Format-B chrome via `topic_core` before comparing to the label.
/// Comparison is whitespace-collapsed + case-blind (`[TG] API · O`
/// covers `api`), and `topic_core` sheds ANY pasted code (stale or
/// current), so kind flips preserve the custom instead of discarding it.
pub fn stored_covers_label(stored: Option<&str>, space: &str, kind: &str, label: &str) -> bool {
    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    }
    stored
        .map(|s| norm(&crate::topics::names::topic_core(s, space, kind)) == norm(label))
        .unwrap_or(false)
}

/// Kind-flip bypass: seen kind differing from current forces reformat
/// (verbatim keeps must not hide an agent→shell `· code` change).
pub fn kind_bypass(last: Option<&str>, cur: &str) -> bool {
    matches!(last, Some(l) if l != cur)
}

/// Reset title decision (single call-site for every reset loop, so the
/// predicate can never drift): 1:1 Format-B always — even a pre-reset
/// stored title equal to the tab core re-renders with space + fresh kind
/// code (user text preserved, never returned bare). Split tabs re-suffix
/// a stored title covering the pane label (fresh kind code, custom
/// preserved — never the tab core, which would orphan it). Pure so
/// unit-tested.
pub fn reset_desired_title(
    pre: Option<&str>,
    space: &str,
    label: &str,
    kind: &str,
    multi: bool,
    pane_label: Option<&str>,
) -> String {
    if !multi && stored_matches_label(pre, label) {
        crate::topics::names::format_title(space, label, kind)
    } else if multi
        && let Some(pl) = pane_label.map(str::trim).filter(|l| !l.is_empty())
        && stored_covers_label(pre, space, kind, pl)
    {
        crate::topics::names::format_title(space, pl, kind)
    } else {
        crate::topics::names::format_title(space, label, kind)
    }
}

#[cfg(test)]
mod tests {
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
        let f = facts(&[("w1:p1", "t1", "a"), ("w1:p2", "t1", "a"), ("w1:p3", "t2", "a")]);
        let c = tab_census(&f);
        assert_eq!(c.get("t1"), Some(&2));
        assert_eq!(c.get("t2"), Some(&1));
        assert_eq!(c.get("t9"), None);
    }

    #[test]
    fn test_tab_of_resolves_split_flag() {
        let f = facts(&[("w1:p1", "t1", "a"), ("w1:p2", "t1", "a"), ("w1:p3", "", "a")]);
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
        // watchdog re-suffixes with the fresh code).
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
        // Raw-stored split custom re-suffixes the pane label (fresh code).
        assert_eq!(
            reset_desired_title(
                Some("[tg] Custom · o"),
                "tg",
                "console o27",
                "opencode",
                true,
                Some("Custom")
            ),
            "[tg] Custom · o"
        );
    }

    #[test]
    fn test_reset_desired_title_verbatim_or_formatted() {
        // 1:1 Format-B always: even a verbatim pre-reset custom gains
        // space + fresh kind code (user text preserved, never bare).
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "My Title", "opencode", false, None),
            "[tg] My Title · o"
        );
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "  My Title  ", "opencode", false, None),
            "[tg] My Title · o"
        );
        // Split tab cores format — a stored title covering the pane
        // label re-suffixes it (custom preserved, fresh kind code).
        assert_eq!(
            reset_desired_title(Some("console o27"), "tg", "console o27", "opencode", true, None),
            "[tg] console o27 · o"
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
            "[tg] Custom Name · o"
        );
        // Formatted otherwise (new/changed cores, case-only changes).
        assert_eq!(
            reset_desired_title(
                Some("[tg] api · o"),
                "tg",
                "backend",
                "opencode",
                false,
                None
            ),
            "[tg] backend · o"
        );
        assert_eq!(
            reset_desired_title(None, "tg", "backend", "opencode", false, None),
            "[tg] backend · o"
        );
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "my title", "opencode", false, None),
            "[tg] my title · o"
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
}
