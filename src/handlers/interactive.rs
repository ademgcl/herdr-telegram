/// Key maps for answering INTERACTIVE blocked panes (see dialog.rs:
/// option buttons, card posts and tap handling live there; this keeps
/// only the static key tables so the file stays under the 300-line cap).
pub fn keys_for(action: &str) -> Option<&'static [&'static str]> {
    match action {
        "allow" => Some(&["enter"]),
        "deny" => Some(&["esc"]),
        "next" => Some(&["right"]),
        _ => None,
    }
}

/// Key sequence answering option N (0-based): Right over N times, Enter.
/// Fresh dialogs highlight the first option. (Was Tab×N: opencode
/// ignores Tab, so every tap confirmed option 1.)
pub fn opt_keys(idx: usize) -> Vec<&'static str> {
    let mut keys = vec!["right"; idx];
    keys.push("enter");
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::model_scan::split_columns;

    #[test]
    fn test_keys_for_actions() {
        assert_eq!(keys_for("allow"), Some(&["enter"][..]));
        assert_eq!(keys_for("deny"), Some(&["esc"][..]));
        assert_eq!(keys_for("next"), Some(&["right"][..]));
        assert_eq!(keys_for("bogus"), None);
    }

    #[test]
    fn test_opt_keys_sequences() {
        assert_eq!(opt_keys(0), vec!["enter"]);
        assert_eq!(opt_keys(1), vec!["right", "enter"]);
        assert_eq!(opt_keys(2), vec!["right", "right", "enter"]);
    }

    #[test]
    fn test_columns_split() {
        assert_eq!(split_columns("Allow once   Allow always   Reject").len(), 3);
        assert_eq!(split_columns("Yes\tNo"), vec!["Yes", "No"]);
        assert_eq!(split_columns("single phrase here"), vec!["single phrase here"]);
        assert_eq!(split_columns("  padded   columns  "), vec!["padded", "columns"]);
    }
}
