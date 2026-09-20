use crate::handlers::dialog::{dialog_sig, parse_options};

/// Post-tap screen: a new dialog, a resumed agent, or an unchanged one.
#[derive(Debug, PartialEq)]
pub enum TapResult {
    NewDialog,
    Resumed,
    Unchanged,
}

/// Pure classification so taps never mislabel a turned-over dialog as
/// "resumed" (the old bug: option sets differed, so `closed` wrongly
/// held) and never strip buttons off a dialog that is still up. Ground
/// truth is fresh herdr status, not screen tea-leaves: `still_blocked`
/// decides resumed; the screens only distinguish a new dialog from an
/// unchanged one. Empty `after` (unreadable screen) never touches the card.
pub fn classify_tap(before: &[String], after: &[String], still_blocked: bool) -> TapResult {
    if !still_blocked {
        return TapResult::Resumed;
    }
    if after.is_empty() {
        return TapResult::Unchanged;
    }
    let opts_before = parse_options(before);
    let opts_after = parse_options(after);
    if opts_after.is_empty() {
        // No options on screen: a changed screen is an option-less
        // second confirm (real dialog — the fallback Confirm button
        // covers it) when the FIRST dialog had none either (text-input
        // turnovers like "Enter name:" → "Enter age:") or when the new
        // screen asks a question. Anything else is vanished-dialog
        // prose while status lags — never a *new* dialog, so a working
        // screen never earns a ghost blocked card with live buttons.
        // Erring to Unchanged is the safe direction (no ghost buttons);
        // the delayed refresh re-checks seconds later.
        if dialog_sig(after) != dialog_sig(before)
            && (opts_before.is_empty() || dialog_sig(after).contains('?'))
        {
            return TapResult::NewDialog;
        }
        return TapResult::Unchanged;
    }
    if opts_after != opts_before || dialog_sig(after) != dialog_sig(before) {
        return TapResult::NewDialog;
    }
    TapResult::Unchanged
}

/// True when a typed answer provably went nowhere: both screens readable
/// and the dialog identical. Unreadable screens never fail the send.
pub fn dialog_stalled(before: &[String], after: &[String]) -> bool {
    !before.is_empty() && !after.is_empty() && dialog_sig(after) == dialog_sig(before)
}

/// True when the screen moved off the tapped dialog: the tap resumed the
/// agent but herdr status still samples `blocked` (lag), so `classify_tap`
/// errs to `Unchanged` (no ghost buttons) while the resumed turn goes
/// watcherless with no final. The caller speculatively follows (which
/// stands down when still blocked), so only vanished turns gain a watcher.
/// Pure for tests.
pub fn dialog_moved(before: &[String], after: &[String]) -> bool {
    !after.is_empty() && dialog_sig(after) != dialog_sig(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_classify_new_dialog_on_turnover() {
        // allow → confirm: option sets differ while still blocked → new
        // card, never "resumed".
        let before = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        let after = v(&["Confirm apply?", "Confirm   Cancel"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
    }

    #[test]
    fn test_classify_new_dialog_when_card_had_none() {
        let before = v(&["△ Permission required"]);
        let after = v(&["Pick a model", "Sonnet   Opus   Haiku"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
    }

    #[test]
    fn test_classify_resumed_when_no_longer_blocked() {
        // Ground truth is status: whatever the screen shows, a moved-on
        // agent means resumed.
        let before = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        let after = v(&["⠋ working…", "editing src/main.rs"]);
        assert_eq!(classify_tap(&before, &after, false), TapResult::Resumed);
    }

    #[test]
    fn test_classify_unchanged_same_dialog() {
        let dlg = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        assert_eq!(classify_tap(&dlg, &dlg, true), TapResult::Unchanged);
        // Unreadable screen never touches the card, either way.
        assert_eq!(classify_tap(&v(&["x"]), &[], true), TapResult::Unchanged);
        assert_eq!(classify_tap(&v(&["x"]), &[], false), TapResult::Resumed);
    }

    #[test]
    fn test_classify_question_only_turnover() {
        // Text inputs have no options: a changed question still counts.
        let before = v(&["Enter name:"]);
        let after = v(&["Enter age:"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
        // Same question, no options: unchanged.
        assert_eq!(classify_tap(&before, &before, true), TapResult::Unchanged);
    }

    #[test]
    fn test_classify_vanished_dialog_is_not_new() {
        // Options gone while status lags blocked: the dialog vanished
        // (working prose), never a new dialog — no ghost card.
        let before = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        let after = v(&["⠋ working…", "editing src/main.rs"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::Unchanged);
    }

    #[test]
    fn test_classify_optionless_turnover_is_new() {
        // Second confirm with no parseable options is still a dialog:
        // short changed screen ⇒ NewDialog (fallback button covers it).
        let before = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        let after = v(&["Apply all edits?", "This cannot be undone."]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::NewDialog);
        // Same option-less dialog twice ⇒ unchanged.
        assert_eq!(classify_tap(&after, &after, true), TapResult::Unchanged);
    }

    #[test]
    fn test_classify_long_prose_turnover_is_not_new() {
        // Long changed prose without options: vanished dialog, not new.
        let before = v(&["Pick one:", "Yes   No"]);
        let after = v(&[&"x".repeat(600)]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::Unchanged);
    }

    #[test]
    fn test_classify_plain_short_prose_is_not_new() {
        // Short prose with no question mark: safe direction is
        // Unchanged (no ghost buttons); the delayed refresh re-checks.
        let before = v(&["Pick one:", "Yes   No"]);
        let after = v(&["Done.", "editing src/main.rs"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::Unchanged);
    }

    #[test]
    fn test_classify_optionless_confirm_without_question_is_unchanged() {
        // No options, no question: Unchanged even when short — the
        // observer/delayed refresh (not a ghost card) handles it.
        let before = v(&["Pick one:", "Yes   No"]);
        let after = v(&["Press Enter to continue"]);
        assert_eq!(classify_tap(&before, &after, true), TapResult::Unchanged);
    }

    #[test]
    fn test_dialog_stalled_same_dialog() {
        let dlg = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        assert!(dialog_stalled(&dlg, &dlg));
        // Typed text echoing into the field counts as progress.
        let filled = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
            "my reason",
        ]);
        assert!(!dialog_stalled(&dlg, &filled));
        // Unreadable screens never fail the send.
        assert!(!dialog_stalled(&[], &dlg));
        assert!(!dialog_stalled(&dlg, &[]));
    }

    #[test]
    fn test_dialog_moved_vanished_vs_stuck() {
        // Resumed but status lags blocked: screen left the dialog (working
        // prose) — the caller must speculatively follow for the final.
        let dlg = v(&[
            "△ Permission required",
            "Allow once   Allow always   Reject",
        ]);
        let working = v(&["⠋ working…", "editing src/main.rs"]);
        assert!(dialog_moved(&dlg, &working));
        // Same dialog: stuck, no follow.
        assert!(!dialog_moved(&dlg, &dlg));
        // Unreadable after: never follow on a blip.
        assert!(!dialog_moved(&dlg, &[]));
    }
}
