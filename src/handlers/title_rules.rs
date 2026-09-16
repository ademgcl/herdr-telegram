//! Pure title-decision rules (no I/O): tab→core picking, verbatim
//! preservation, and the reset Step-4 choice. Split from `titles`
//! (300-line file limit).

/// Pick the bare core for a topic title: user-visible tab name, else
/// the stable tag. Terminal/agent titles are NEVER used here — they live
/// only in the pinned identity card. Multi-pane tabs disambiguate with
/// the tag (`console` + `o27` → `console o27`), so split siblings never
/// collide. Empty/whitespace names count as missing (herdr tab names are
/// never blank in practice — this is just the safety net).
pub fn pick_core(tab: Option<&str>, tag: &str, multi: bool) -> String {
    if let Some(t) = tab.map(str::trim).filter(|t| !t.is_empty()) {
        if multi {
            return format!("{t} {tag}");
        }
        return t.to_string();
    }
    tag.to_string()
}

/// Verbatim-preservation predicate: a stored topic title that already
/// equals the herdr tab name (trim-compared) means a Telegram native
/// rename just synced both sides — the watchdog must keep it exactly,
/// never reformat. Pure so it is unit-tested, not just eyeballed.
pub fn stored_matches_label(stored: Option<&str>, label: &str) -> bool {
    stored.map(str::trim) == Some(label.trim())
}

/// Reset Step-4 title decision (single call-site for both agent + shell
/// loops, so the predicate can never drift): a pre-reset stored title
/// equal to the herdr label means the user set it verbatim — re-apply
/// raw, else format. Pure so it is unit-tested.
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
        assert_eq!(pick_core(Some("agy_gelistirme"), "a14", false), "agy_gelistirme");
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
        // Verbatim when pre-reset stored equals the herdr label.
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "My Title", "opencode"),
            "My Title"
        );
        assert_eq!(
            reset_desired_title(Some("My Title"), "tg", "  My Title  ", "opencode"),
            "My Title"
        );
        // Formatted otherwise (new/changed labels, case-only changes).
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
}
