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
    if parse_options(after) != parse_options(before) {
        return TapResult::NewDialog;
    }
    TapResult::Unchanged
}

/// True when a typed answer provably went nowhere: both screens readable
/// and the dialog identical. Unreadable screens never fail the send.
pub fn dialog_stalled(before: &[String], after: &[String]) -> bool {
    !before.is_empty() && !after.is_empty() && dialog_sig(after) == dialog_sig(before)
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
}
