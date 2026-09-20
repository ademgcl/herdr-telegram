//! Dialog card-guard tests (sig, blank-outage, button caps). Split
//! from `tests` (300-line file limit).
use super::{blocked_kb, dialog_sig, is_blank_card, parse_options};

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_dialog_sig_covers_option_turnover() {
    // Same question, different options → different sig (an option-only
    // turnover must repost, never go silent with stale buttons).
    let a = v(&["Allow once   Allow always   Reject"]);
    let b = v(&["Allow once   Deny"]);
    assert_ne!(dialog_sig(&a), dialog_sig(&b));
    assert_eq!(dialog_sig(&a), dialog_sig(&a));
}

#[test]
fn test_is_blank_card_guards_outage_keeps_pick() {
    // Whitespace-only/outage read: no buzz. Blank question WITH
    // options is a real picker and still posts.
    assert!(is_blank_card(&v(&["   "])));
    assert!(is_blank_card(&[]));
    assert!(!is_blank_card(&v(&["Allow once   Deny"])));
}

#[test]
fn test_parse_options_numbered_caps_at_8() {
    // Scrollback lists never mint unreachable buttons (card+tap bound 8).
    let names = [
        "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    let lines: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{}. {n}", i + 1))
        .collect();
    let opts = parse_options(&lines);
    assert_eq!(opts.len(), 8, "uncapped parse: {opts:?}");
    assert_eq!(opts[7], "Eight");
}

#[test]
fn test_blocked_kb_8_options_wrap_two_rows() {
    // 8 options lay out as 2 rows of 4 + the dismiss row; a 9th
    // option never mints a button.
    let opts: Vec<String> = (1..=9).map(|i| format!("Opt{i}")).collect();
    let kb = blocked_kb("w1:p1", &opts);
    let rows = kb.as_array().unwrap();
    assert_eq!(rows.len(), 3, "2 option rows + dismiss: {kb}");
    assert_eq!(rows[0].as_array().unwrap().len(), 4);
    assert_eq!(rows[1].as_array().unwrap().len(), 4);
    assert_eq!(rows[1][3]["callback_data"], "B:opt7:w1:p1");
    assert!(!kb.to_string().contains("opt8"), "9th option minted: {kb}");
    // 5 options wrap 4 + 1, never a single row of 5.
    let opts5: Vec<String> = (1..=5).map(|i| format!("Opt{i}")).collect();
    let kb5 = blocked_kb("w1:p1", &opts5);
    let rows5 = kb5.as_array().unwrap();
    assert_eq!(rows5.len(), 3, "4 + 1 + dismiss: {kb5}");
    assert_eq!(rows5[0].as_array().unwrap().len(), 4);
    assert_eq!(rows5[1].as_array().unwrap().len(), 1);
}
