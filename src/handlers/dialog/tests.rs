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

#[test]
fn test_parse_options_numbered_list() {
    let lines = v(&[
        "❯ 1. Discard",
        "  2. Keep",
        "  3. Test",
        "  4. Commit",
        "────────────────────────────────",
        "  5. Chat about this",
    ]);
    assert!(super::has_numbered_options(&lines));
    assert_eq!(
        parse_options(&lines),
        vec!["Discard", "Keep", "Test", "Commit", "Chat about this"]
    );
}

#[test]
fn test_live_card_claude_picker_d1() {
    // D1: live wA:pG capture — Claude AskUserQuestion picker
    let screen = v(&[
        "← ☐ Partial ☐ Story ✔ Submit →",
        "│ What would you like to do with changes?",
        "❯ 1. Discard",
        "  2. Keep",
        "  3. Test",
        "  4. Commit",
        "────────────────────────────────────────",
        "  5. Chat about this",
        "Enter to select · Tab/Arrow keys to navigate",
    ]);
    let (q, opts) = super::live_card(&screen);
    assert!(
        q.contains("What would you like to do"),
        "question intact: {q}"
    );
    assert!(
        !q.contains("Enter to select"),
        "hint row filtered: {q}"
    );
    assert_eq!(
        opts,
        vec!["Discard", "Keep", "Test", "Commit", "Chat about this"]
    );
    let kb = blocked_kb("wA:pG", &opts);
    let row = kb.as_array().unwrap()[0].as_array().unwrap();
    assert_eq!(row.len(), 4);
    assert_eq!(row[0]["text"], "1: Discard");
    assert_eq!(row[1]["text"], "2: Keep");
}

#[test]
fn test_live_card_opencode_permission_wide_terminal_d2() {
    // D2: live w5:p1 capture — OpenCode permission dialog on wide terminal
    let screen = v(&[
        "  ┃  △ Permission required",
        "  ┃    ← Access external directory ~/.config/opencode",
        "  ┃",
        "  ┃  Patterns",
        "  ┃  - /home/user/.config/opencode/*",
        "  ┃",
        "  ┃   Allow once   Allow always   Reject                                                                                 ctrl+f fullscreen  ⇆ select  enter confirm",
    ]);
    let (q, opts) = super::live_card(&screen);
    assert!(
        q.contains("Permission required"),
        "header intact: {q}"
    );
    assert!(
        q.contains("Access external directory"),
        "access path intact: {q}"
    );
    assert_eq!(
        opts,
        vec!["Allow once", "Allow always", "Reject"]
    );
}

