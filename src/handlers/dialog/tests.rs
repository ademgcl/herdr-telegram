//! Dialog parser tests. Split from `dialog` (300-line file limit).
use super::{blocked_kb, parse_options};

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_parse_options_permission_row() {
    let lines = v(&[
        "△ Permission required",
        "Patterns",
        "- /home/user/.config/opencode/*",
        "Allow once   Allow always   Reject",
        "ctrl+f fullscreen  ⇆ select  enter confirm",
    ]);
    assert_eq!(
        parse_options(&lines),
        vec!["Allow once", "Allow always", "Reject"]
    );
}

#[test]
fn test_parse_options_skips_hints_and_prose() {
    assert!(parse_options(&v(&["ctrl+f fullscreen  enter confirm"])).is_empty());
    assert!(parse_options(&v(&["Hello! How can I help you today?"])).is_empty());
    assert!(parse_options(&v(&["| a | b |", "| c | d |"])).is_empty());
    assert!(parse_options(&v(&["first line", "second line"])).is_empty());
}

#[test]
fn test_parse_options_framed_row() {
    // Tap paths parse the raw visible screen (rails intact) — a
    // framed row must yield the same options as card text.
    let lines = v(&[
        "┃ △ Permission required",
        "┃   Allow once   Allow always   Reject",
    ]);
    assert_eq!(
        parse_options(&lines),
        vec!["Allow once", "Allow always", "Reject"]
    );
}

#[test]
fn test_parse_options_merged_hint_row() {
    // Wide terminals merge options + right-aligned hints on ONE row
    // (live opencode 1.18 dialog) — the real options must survive.
    let lines = v(&[
        "  ┃   Allow once   Allow always   Reject                                                                                 ctrl+f fullscreen  ⇆ select  enter confirm",
    ]);
    assert_eq!(
        parse_options(&lines),
        vec!["Allow once", "Allow always", "Reject"]
    );
    // Two-option dialog with merged hints.
    let lines = v(&["Confirm   Cancel   ctrl+f fullscreen  enter confirm"]);
    assert_eq!(parse_options(&lines), vec!["Confirm", "Cancel"]);
}

#[test]
fn test_parse_options_capitalized_hint_words_survive() {
    // Real options carrying hint substrings are capitalized;
    // hint chrome is lowercase.
    let lines = v(&["Go back   Describe   Dismiss changes"]);
    assert_eq!(
        parse_options(&lines),
        vec!["Go back", "Describe", "Dismiss changes"]
    );
    // Lowercase hint rows still filter out.
    assert!(parse_options(&v(&["go back   esc close"])).is_empty());
}

#[test]
fn test_parse_options_single_spaced_confirms() {
    // Minimal second confirms without column padding.
    assert_eq!(
        parse_options(&v(&["Confirm Cancel"])),
        vec!["Confirm", "Cancel"]
    );
    // Prose never matches the closed vocab.
    assert!(parse_options(&v(&["Hello world"])).is_empty());
    assert!(parse_options(&v(&["ok continue working"])).is_empty());
}

#[test]
fn test_parse_options_vertical_stack() {
    let lines = v(&["Apply to all files?", "Yes", "No"]);
    assert_eq!(parse_options(&lines), vec!["Yes", "No"]);
    // Singletons and prose runs don't parse.
    assert!(parse_options(&v(&["Yes"])).is_empty());
    assert!(parse_options(&v(&["Hello", "World"])).is_empty());
}

#[test]
fn test_callback_data_survives_pane_colons() {
    let kb = blocked_kb("wG:p1", &v(&["Allow once"]));
    let data = kb[0][0]["callback_data"].as_str().unwrap();
    let route: Vec<&str> = data.splitn(3, ':').collect();
    assert_eq!(route, vec!["B", "opt0", "wG:p1"]);
}
