//! Blocked-dialog option-row parser: which TUI rows are tappable
//! answers vs key-hint chrome. Pure (no I/O) so it is unit-tested.
//! Split from `dialog` (300-line file limit).
use crate::{
    handlers::model_scan::split_columns,
    jobs::{filter::deframe, segment::is_rule},
};

fn parse_numbered_option(line: &str) -> Option<(usize, String)> {
    let t = deframe(line);
    let s = t.trim().trim_start_matches(['❯', '*', '-']).trim();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let num: usize = digits.parse().ok()?;
    let rest = s[digits.len()..].trim_start();
    let after = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    let label = after.trim();
    if label.is_empty() {
        return None;
    }
    let clean_label = if let Some(idx) = label.find("   ") {
        label[..idx].trim()
    } else {
        label
    };
    if clean_label.is_empty() {
        return None;
    }
    Some((num, clean_label.to_string()))
}

pub fn has_numbered_options(lines: &[String]) -> bool {
    let mut expected = 1;
    let mut count = 0;
    for line in lines {
        let t = deframe(line);
        if t.is_empty() || is_rule(&t) {
            continue;
        }
        if let Some((n, _)) = parse_numbered_option(line)
            && n == expected
        {
            count += 1;
            expected += 1;
            if count >= 2 {
                return true;
            }
            continue;
        }
        expected = 1;
        count = 0;
        if let Some((1, _)) = parse_numbered_option(line) {
            expected = 2;
            count = 1;
        }
    }
    false
}

/// Lines that can never be options: key-hint rows and chrome-ish labels.
/// NOTE: "confirm"/"cancel" are deliberately absent — real second
/// dialogs read "Confirm   Cancel", and hint rows carrying those words
/// always also carry ctrl/enter/esc/select (filtered on those instead).
/// Matching is case-aware: hint chrome is lowercase ("enter confirm")
/// while real options are capitalized ("Go back", "Describe", "Escape",
/// "Dismiss changes") — case breaks the tie. A lowercase option that
/// happens to contain a hint word is the accepted miss.
const HINT_WORDS: &[&str] = &[
    "ctrl",
    "enter",
    "select",
    "fullscreen",
    "esc",
    "shortcut",
    "navigate",
    "dismiss",
    "back",
    "quit",
];

/// Closed vocab for minimal confirms (single-spaced pairs, vertical
/// stacks). Only exact words — prose never matches.
const CONFIRM_WORDS: &[&str] = &[
    "confirm", "cancel", "ok", "yes", "no", "y", "n", "allow", "deny", "reject", "retry", "abort",
    "continue",
];

fn is_confirm_word(w: &str) -> bool {
    CONFIRM_WORDS.iter().any(|v| v == &w.to_lowercase())
}

/// Selectable option labels out of a question card: the first line
/// holding 2–4 short phrases in columns ("Allow once   Allow always
/// Reject"). De-frames first: tap paths parse the raw visible screen
/// (frame rails intact), so a framed row must parse exactly like the
/// de-framed card text. Wide terminals merge the option row with the
/// right-aligned hint row on ONE terminal line
/// ("Allow once   Allow always   Reject   ctrl+f fullscreen …"), so hint
/// columns are filtered per-column — never by discarding the whole line.
/// Pure — tested in tests.rs.
pub fn parse_options(lines: &[String]) -> Vec<String> {
    for line in lines {
        let t = deframe(line);
        if t.is_empty() {
            continue;
        }
        let parts = split_columns(&t);
        // Drop key-hint columns (right-aligned hints share the option
        // row on wide terminals); the remaining real options must be 2–4
        // short phrases. Hint-only rows filter to empty and are skipped.
        let real: Vec<String> = parts
            .into_iter()
            .filter(|p| {
                let low = p.to_lowercase();
                let hinty = HINT_WORDS.iter().any(|w| low.contains(w));
                !hinty || p.chars().any(|c| c.is_uppercase())
            })
            .collect();
        let short = real.len() >= 2
            && real.len() <= 4
            && real.iter().all(|p| {
                let len = p.chars().count();
                (2..=24).contains(&len) && p.split_whitespace().count() <= 3
            });
        if short {
            return real;
        }
        // Single-spaced minimal confirms ("Confirm Cancel"): every word
        // must be closed-vocab, so prose never matches.
        let words: Vec<&str> = t.split_whitespace().collect();
        if (2..=4).contains(&words.len()) && words.iter().all(|w| is_confirm_word(w)) {
            return words.iter().map(|w| w.to_string()).collect();
        }
    }
    // Vertical stacks ("Yes" / "No" on consecutive lines): runs of
    // single vocab words. Blank lines don't break a stack; any other
    // line ends the run (returned only at 2–4 length, like columns).
    let mut run: Vec<String> = Vec::new();
    for line in lines {
        let t = deframe(line);
        if t.is_empty() {
            continue;
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        if words.len() == 1 && is_confirm_word(words[0]) {
            run.push(words[0].to_string());
            continue;
        }
        if (2..=4).contains(&run.len()) {
            return std::mem::take(&mut run);
        }
        run.clear();
    }
    if (2..=4).contains(&run.len()) {
        return run;
    }
    // Numbered option runs ("1. Discard", "2. Keep", "3. Test"):
    // capped at 8 (card + tap bound above) so a long numbered list in
    // scrollback never mints unreachable buttons.
    let mut num_run: Vec<String> = Vec::new();
    let mut expected_num = 1;
    for line in lines {
        let t = deframe(line);
        if t.is_empty() || is_rule(&t) {
            continue;
        }
        if let Some((n, label)) = parse_numbered_option(line)
            && n == expected_num
        {
            num_run.push(label);
            expected_num += 1;
            if num_run.len() >= 8 {
                return num_run;
            }
            continue;
        }
        if num_run.len() >= 2 {
            return num_run;
        }
        num_run.clear();
        expected_num = 1;
        if let Some((1, label)) = parse_numbered_option(line) {
            num_run.push(label);
            expected_num = 2;
        }
    }
    if num_run.len() >= 2 {
        return num_run;
    }
    Vec::new()
}
