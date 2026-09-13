/// Free-Zen shortlist + footer parsing for the opencode `/models` picker.
/// Pure — tested below. Live picker scanning lives in `model_scan`,
/// async driving in `model`.

/// Free Zen models: display name EXACTLY as the `/models` picker shows it,
/// id slug, and a short button label (Telegram buttons truncate past ~22
/// chars, so the full names don't fit — the tap maps by INDEX, so the
/// short text is display-only and can never mis-route).
/// Picker-grounded (verified live) — the TUI shortens some docs names
/// (e.g. no "Contributor"), and filters/markers must match the picker
/// text, not the docs.
/// Config form would be `opencode/<id>`; the picker matches on names.
pub const FREE_ZEN: &[(&str, &str, &str)] = &[
    ("Big Pickle", "big-pickle", "Big Pickle"),
    ("Muse Spark 1.3 Free", "muse-spark-1.3-free", "Muse Spark 1.3"),
    (
        "Ling 3.0 Flash Fin Free",
        "ling-3.0-flash-fin-free",
        "Ling 3.0 Flash",
    ),
    (
        "Nemotron 3.5 Lightning Free",
        "nemotron-3.5-lightning-free",
        "Nemotron 3.5",
    ),
    (
        "Nemotron 3 Ultra Free",
        "nemotron-3-ultra-free",
        "Nemotron 3 Ultra",
    ),
    ("MiMo V2.5 Free", "mimo-v2.5-free", "MiMo V2.5"),
    ("Muse Spark 1.2 Free", "muse-spark-1.2-free", "Muse Spark 1.2"),
];

pub(crate) fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Current model label off a TUI screen: the LAST `Build · …` / `Plan · …`
/// footer line's middle (`Plan · Ling … Zen · high` → `Ling … Zen`).
pub fn footer_model(screen: &[String]) -> Option<String> {
    let mut found = None;
    for line in screen {
        let t = line.trim().trim_start_matches(['┃', '│', '|', '▣', ' ']);
        for head in ["Build ·", "Plan ·"] {
            if let Some(rest) = t.strip_prefix(head) {
                // Middle segment(s): everything up to the last '·'
                // (trailing variant / duration).
                if let Some((mid, _)) = rest.rsplit_once('·') {
                    let label = mid.trim().to_string();
                    if label.len() >= 3 {
                        found = Some(label);
                    }
                }
            }
        }
    }
    found
}

/// Filter text isolating a model: full display name + zen.
pub fn tap_filter(name: &str) -> String {
    format!("{} zen", name.to_lowercase())
}

/// Resolve a free-list tap index to (filter, marker).
pub fn free_tap(idx: usize) -> Option<(String, String)> {
    FREE_ZEN
        .get(idx)
        .map(|(name, _, _)| (tap_filter(name), name.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_footer_model_variants() {
        let build = v(&["  ┃  Build · Muse Spark 1.3 Free OpenCode Zen · xhigh"]);
        assert_eq!(
            footer_model(&build).as_deref(),
            Some("Muse Spark 1.3 Free OpenCode Zen")
        );
        let plan = v(&["┃  Plan · Ling 3.0 Flash Fin Free OpenCode Zen · high"]);
        assert_eq!(
            footer_model(&plan).as_deref(),
            Some("Ling 3.0 Flash Fin Free OpenCode Zen")
        );
        // Last match wins (status bar beats step lines).
        let both = v(&[
            "▣  Plan · Muse Spark 1.3 Free · 31.6s",
            "┃  Plan · Ling 3.0 Flash Fin Free OpenCode Zen · high",
        ]);
        assert_eq!(
            footer_model(&both).as_deref(),
            Some("Ling 3.0 Flash Fin Free OpenCode Zen")
        );
        assert_eq!(footer_model(&v(&["nothing here"])), None);
    }

    #[test]
    fn test_tap_filter_and_free_tap() {
        assert_eq!(tap_filter("Big Pickle"), "big pickle zen");
        let (filter, marker) = free_tap(0).unwrap();
        assert_eq!(filter, "big pickle zen");
        assert_eq!(marker, "big pickle");
        assert!(free_tap(99).is_none());
    }

    #[test]
    fn test_model_card_lists_free_shortlist() {
        let text = super::super::model::model_card_text(
            Some("Ling 3.0 Flash Fin Free OpenCode Zen"),
            "w1:p1",
            "opencode",
        );
        assert!(text.contains("now using: Ling 3.0 Flash Fin Free OpenCode Zen"));
        assert!(text.contains("Big Pickle"));
        assert!(text.contains("/model"));
    }
}
