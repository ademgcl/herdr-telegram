//! Dialog parser tests. Split from `dialog` (300-line file limit).
use super::{
    blocked_card_text, blocked_kb, parse_options, winner_lines,
};
use super::surfaces::{settle_select, track_card};
use std::collections::HashMap;

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

#[test]
fn test_track_card_replaces_same_chat_appends_others() {
    let mut map: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
    assert_eq!(track_card(&mut map, "w1:p1", 1, 101), None);
    assert_eq!(track_card(&mut map, "w1:p1", 2, 201), None);
    // Repost in the same chat replaces and reports the evicted
    // surface, so the caller strips it: resolve strips each live
    // surface exactly once, never a stale mid.
    assert_eq!(track_card(&mut map, "w1:p1", 1, 102), Some((1, 101)));
    assert_eq!(map["w1:p1"], vec![(1, 102), (2, 201)]);
    // Other panes are untouched.
    assert_eq!(track_card(&mut map, "w1:p2", 1, 301), None);
    assert_eq!(map["w1:p2"], vec![(1, 301)]);
}

#[test]
fn test_settle_select_replaces_and_orphans_siblings() {
    // Same-chat repost: predecessor stale, live surface kept.
    // (Sibling entries keep scan order; order is irrelevant.)
    let (keep, stale) = settle_select(vec![(1, 101), (2, 201)], 1, 102);
    assert_eq!(keep, vec![(2, 201), (1, 102)]);
    assert_eq!(stale, vec![(1, 101), (2, 201)]);
    // Same surface again: pure no-op re-ensure.
    let (keep, stale) = settle_select(vec![(1, 102)], 1, 102);
    assert_eq!(keep, vec![(1, 102)]);
    assert!(stale.is_empty());
    // Restart-emptied map: live surface tracked from nothing.
    let (keep, stale) = settle_select(vec![], 1, 102);
    assert_eq!(keep, vec![(1, 102)]);
    assert!(stale.is_empty());
}

#[test]
fn test_type_button_only_without_options() {
    // Select dialog: options navigate, free text has nowhere to land.
    let kb = blocked_kb("w1:p1", &v(&["Allow once", "Deny"]));
    let flat = kb.to_string();
    assert!(!flat.contains("Type answer"), "select must not offer Type");
    assert!(flat.contains("Dismiss"), "Dismiss always stays");
    // Text input: no options, Type is the only way to answer in chat.
    let kb = blocked_kb("w1:p1", &[]);
    let flat = kb.to_string();
    assert!(flat.contains("Type answer"), "text input needs Type");
    assert!(flat.contains("Confirm"), "fallback Confirm stays");
}

#[test]
fn test_blocked_card_text_promises_typing_only_without_options() {
    let t = blocked_card_text("Pick?", &v(&["Yes", "No"]));
    assert!(t.contains("Tap an answer."));
    assert!(!t.contains("type it"));
    let t = blocked_card_text("Name?", &[]);
    assert!(t.contains("or just type it"));
}

#[test]
fn test_winner_lines_ignores_scrollback_options() {
    // Stale numbered list above a rule, live text question below: the
    // text gate must see no options (same basis as tap bounds checks).
    let screen = v(&[
        "1. Old choice",
        "2. Other choice",
        "────────────────────────────────",
        "Enter a name:",
    ]);
    let win = winner_lines(&screen);
    assert!(
        parse_options(&win).is_empty(),
        "scrollback options must not gate text: {win:?}"
    );
    assert!(winner_lines(&[]).is_empty());
}

#[test]
fn test_dialog_sig_covers_option_turnover() {
    // Same question, different options → different sig (an option-only
    // turnover must repost, never go silent with stale buttons).
    let a = v(&["Allow once   Allow always   Reject"]);
    let b = v(&["Allow once   Deny"]);
    assert_ne!(super::dialog_sig(&a), super::dialog_sig(&b));
    assert_eq!(super::dialog_sig(&a), super::dialog_sig(&a));
}

#[test]
fn test_is_blank_card_guards_outage_keeps_pick() {
    // Whitespace-only/outage read: no buzz. Blank question WITH
    // options is a real picker and still posts.
    assert!(super::is_blank_card(&v(&["   "])));
    assert!(super::is_blank_card(&[]));
    assert!(!super::is_blank_card(&v(&["Allow once   Deny"])));
}

