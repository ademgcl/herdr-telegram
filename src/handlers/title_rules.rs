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

/// Pick the bare core for a topic title: user-visible tab name, else
/// the stable tag. Terminal/agent titles are NEVER used here — they live
/// only in the pinned identity card. Multi-pane tabs disambiguate with
/// the tag (`console` + `o27` → `console o27`), so split siblings never
/// collide. Empty/whitespace names count as missing (herdr tab names are
/// never blank in practice — this is just the safety net).
pub fn pick_core(tab: Option<&str>, tag: &str, multi: bool) -> String {
    naming_core(tab, tag, multi).unwrap_or_else(|| tag.to_string())
}

/// Verbatim-preservation predicate: a stored topic title that already
/// equals the herdr tab name (trim-compared) means a Telegram native
/// rename just synced both sides — the watchdog must keep it exactly,
/// never reformat. Pure so it is unit-tested, not just eyeballed.
pub fn stored_matches_label(stored: Option<&str>, label: &str) -> bool {
    stored.map(str::trim) == Some(label.trim())
}

/// Reset title decision (single call-site for every reset loop, so the
/// predicate can never drift): a pre-reset stored title equal to the
/// tab core means the user set it verbatim — re-apply raw, else format.
/// Pure so it is unit-tested.
pub fn reset_desired_title(pre: Option<&str>, space: &str, label: &str, kind: &str) -> String {
    if stored_matches_label(pre, label) {
        label.trim().to_string()
    } else {
        crate::topics::names::format_title(space, label, kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_core_prefers_tab() {
        // User-visible tab name wins; empty/missing falls back to tag.
        // Terminal titles never reach here (pinned card only).
        assert_eq!(
            pick_core(Some("agy_gelistirme"), "a14", false),
            "agy_gelistirme"
        );
        assert_eq!(pick_core(Some("console"), "o27", false), "console");
        // Split tabs disambiguate with the tag.
        assert_eq!(pick_core(Some("console"), "o27", true), "console o27");
        // No tab: stable tag default. Blank counts as missing.
        assert_eq!(pick_core(None, "o1", false), "o1");
        assert_eq!(pick_core(Some("  "), "sh1", false), "sh1");
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
    fn test_reset_desired_title_verbatim_or_formatted() {
        // Verbatim when pre-reset stored equals the tab core.
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "My Title", "opencode"),
            "My Title"
        );
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "  My Title  ", "opencode"),
            "My Title"
        );
        // Formatted otherwise (new/changed cores, case-only changes).
        assert_eq!(
            reset_desired_title(Some("[tg] api · opencode"), "tg", "backend", "opencode"),
            "[tg] backend · opencode"
        );
        assert_eq!(
            reset_desired_title(None, "tg", "backend", "opencode"),
            "[tg] backend · opencode"
        );
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "my title", "opencode"),
            "[tg] my title · opencode"
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
}
