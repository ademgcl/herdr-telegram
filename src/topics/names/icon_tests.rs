//! Tests for per-kind topic icons (split: 300-line file limit).
use super::*;

#[test]
fn test_context_icon_mapping() {
    // Frozen compat: shell keeps 💬 (shell detector cross-checks it),
    // opencode keeps 💻.
    assert_eq!(context_icon_emoji_id("shell"), "5417915203100613993");
    assert_eq!(context_icon_emoji_id("opencode"), "5350554349074391003");
    // Per-agent glyphs + generic fallback (never the shell glyph).
    assert_eq!(context_icon_emoji_id("agy"), "5309832892262654231");
    assert_eq!(context_icon_emoji_id("claude"), "5237889595894414384");
    assert_eq!(context_icon_emoji_id("codex"), "5373251851074415873");
    assert_eq!(context_icon_emoji_id("gemini"), "5350367161514732241");
    assert_eq!(context_icon_emoji_id("cursor"), "5357121491508928442");
    assert_eq!(context_icon_emoji_id("droid"), "5309832892262654231");
    assert_eq!(context_icon_emoji_id("?"), "5309832892262654231");
    assert_eq!(context_icon_emoji_id("SHELL"), "5417915203100613993");
}

#[test]
fn test_icon_table_ids_unique() {
    // Distinct glyphs per kind (fallback may repeat as the default).
    let mut seen = std::collections::HashSet::new();
    for (_, id) in KIND_ICONS {
        assert!(seen.insert(id), "icon reused: {id}");
    }
}

#[test]
fn test_is_bot_icon() {
    assert!(is_bot_icon("5350554349074391003"));
    assert!(is_bot_icon("5417915203100613993"));
    assert!(is_bot_icon("5309832892262654231"));
    assert!(!is_bot_icon("1234567890"));
    assert!(!is_bot_icon(""));
}

#[test]
fn test_icon_needs_update() {
    // Flip since last write → new glyph.
    assert_eq!(
        icon_needs_update(Some("5350554349074391003"), "shell"),
        Some("5417915203100613993")
    );
    assert_eq!(
        icon_needs_update(Some("5417915203100613993"), "agy"),
        Some("5309832892262654231")
    );
    // Current, unknown, and user customs → no write. Missing heals
    // (a create-time icon RPC failure must not keep the default glyph
    // forever — sync_inner/creation both converge on it).
    assert_eq!(icon_needs_update(Some("5417915203100613993"), "shell"), None);
    assert_eq!(icon_needs_update(None, "agy"), Some("5309832892262654231"));
    assert_eq!(icon_needs_update(Some("5350554349074391003"), "?"), None);
    assert_eq!(icon_needs_update(Some("1234567890"), "agy"), None);
    assert_eq!(icon_needs_update(Some(""), "agy"), None);
}

#[test]
fn test_check_context_icons() {
    let valid: Vec<String> = KIND_ICONS
        .iter()
        .map(|(_, id)| id.to_string())
        .chain(std::iter::once(FALLBACK_ICON.to_string()))
        .chain(std::iter::once("1234567890".to_string()))
        .collect();
    assert!(check_context_icons(&valid).is_empty());

    let missing_one: Vec<String> = valid
        .into_iter()
        .filter(|s| s != "5350554349074391003")
        .collect();
    assert_eq!(
        check_context_icons(&missing_one),
        vec!["5350554349074391003"]
    );

        let none_valid: Vec<String> = vec![];
        assert_eq!(check_context_icons(&none_valid).len(), KIND_ICONS.len());
}
