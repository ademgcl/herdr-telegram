//! Provider-agnostic reply extraction: the fresh reply is the last
//! non-empty output segment after cycle boundaries and prompt echoes.
//!
//! herdr exposes only raw terminal text for every agent; earlier turns and
//! intermediate work are dropped here so cards carry just what was said.

use super::{
    echo::{echo_rest, is_echo_continuation, is_input_opener, is_prompt_echo},
    filter::chrome_filtered,
};

/// Tool-call echo prefixes across providers (opencode →/←, claude ●/○/⏺/⎿,
/// codex/pi ☰/❯ …): after one of these, prior prose is intermediate work.
const TOOL_PREFIXES: &[&str] = &["→", "←", "●", "○", "⏺", "⎿", "☰", "❯", "›"];

/// A full-width box-rule turn separator (────…): turns are wrapped in these,
/// so the fresh reply follows the last one. ASCII "---" is deliberately NOT
/// a rule — markdown answers use it as content.
pub fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 8
        && t.chars()
            .all(|c| c.is_whitespace() || matches!(c, '─' | '━' | '═' | '╍'))
        && t.contains(&['─', '━', '═', '╍'][..])
}

pub(crate) fn is_numbered_option_line(s: &str) -> bool {
    let t = s.trim().trim_start_matches('❯').trim_start();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &t[digits..];
        rest.starts_with('.') || rest.starts_with(')')
    } else {
        false
    }
}

fn is_continuation_numbered_option(s: &str) -> bool {
    let t = s.trim().trim_start_matches('❯').trim_start();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if let Ok(num) = digits.parse::<usize>()
        && num > 1
    {
        let rest = &t[digits.len()..];
        return rest.starts_with('.') || rest.starts_with(')');
    }
    false
}

/// A line that opens a new output cycle: tool-call echoes, reasoning
/// headers, step footers, turn rules. The agent's fresh reply is whatever
/// follows the LAST one — earlier segments are intermediate work, not the
/// answer. (Deliberately narrow: todo/log lines and `>` quotes can appear
/// inside answers, so they never split.)
pub fn is_boundary(line: &str) -> bool {
    if is_rule(line) {
        return true;
    }
    let t = line.trim();
    if t.starts_with('❯') && is_numbered_option_line(t) {
        return false;
    }
    if TOOL_PREFIXES.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    if t.starts_with("Thought")
        || t.starts_with("+ Thought")
        || t.starts_with("▸ Thought")
        || t.starts_with("Thinking")
        || t.starts_with("Working…")
        || t.starts_with("Working...")
        || t.starts_with("Click to expand")
    {
        return true;
    }
    t.contains('▣') || t.contains("Build ·")
}

pub fn is_dialog_boundary(line: &str, next_line: Option<&str>) -> bool {
    if is_rule(line) {
        if let Some(next) = next_line
            && is_continuation_numbered_option(next)
        {
            return false;
        }
        return true;
    }
    let t = line.trim();
    if t.starts_with('❯') && is_numbered_option_line(t) {
        return false;
    }
    // In dialog mode, headers live behind '←' (← Access ..., ← ☐ Partial ...)
    if t.starts_with('←') {
        return false;
    }
    if TOOL_PREFIXES.iter().any(|p| *p != "←" && t.starts_with(p)) {
        return true;
    }
    if t.starts_with("Thought")
        || t.starts_with("+ Thought")
        || t.starts_with("▸ Thought")
        || t.starts_with("Thinking")
        || t.starts_with("Working…")
        || t.starts_with("Working...")
        || t.starts_with("Click to expand")
    {
        return true;
    }
    t.contains('▣') || t.contains("Build ·")
}

fn is_footer_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with('⏵') || t.contains("auto mode on") || t.contains("shift+tab to cycle") {
        return true;
    }
    if t.contains("· ←") && t.ends_with("agent") {
        return true;
    }
    if (t.contains(" · ~/") || t.contains(" · /"))
        && (t.starts_with("gpt-")
            || t.starts_with("claude-")
            || t.starts_with("gemini-")
            || t.starts_with("o1-")
            || t.starts_with("o3-")
            || t.contains("medium ·")
            || t.contains("high ·")
            || t.contains("low ·"))
    {
        return true;
    }
    if t.contains("Enter to select")
        || t.contains("Tab/Arrow")
        || t.contains("ctrl+f fullscreen")
        || t.contains("? for shortcuts")
    {
        return true;
    }
    false
}

pub(crate) fn is_footer_segment(seg: &[String]) -> bool {
    if seg.is_empty() || seg.len() > 2 {
        return false;
    }
    seg.iter().all(|l| is_footer_line(l))
}

/// Fresh reply = chrome-cleaned last non-empty segment after cycle
/// boundaries and prompt echoes. A trailing footer/input box yields an
/// empty tail, so the last NON-EMPTY segment wins instead of blanking.
pub fn final_block(lines: &[String], prompt: &str) -> Vec<String> {
    let want = prompt.lines().next().map(str::trim).unwrap_or("");
    let rest = echo_rest(prompt);
    let mut cands: Vec<(Vec<String>, bool)> = Vec::new();
    let mut seg: Vec<String> = Vec::new();
    let mut echo_pos: Option<usize> = None;
    let flush =
        |seg: &mut Vec<String>, cands: &mut Vec<(Vec<String>, bool)>, closed_by_rule: bool| {
            let cand = chrome_filtered(seg);
            if !cand.is_empty() {
                cands.push((cand, closed_by_rule));
            }
            seg.clear();
        };
    for l in lines.iter() {
        if is_boundary(l) {
            flush(&mut seg, &mut cands, is_rule(l));
            echo_pos = None;
            continue;
        }
        if !want.is_empty() && is_prompt_echo(l, want) {
            flush(&mut seg, &mut cands, false);
            echo_pos = Some(0);
            continue;
        }
        if let Some(pos) = echo_pos {
            if l.trim().is_empty() {
                continue;
            }
            if is_echo_continuation(l, &rest, pos) {
                echo_pos = Some(pos + 1);
                continue;
            }
            echo_pos = None;
        }
        seg.push(l.clone());
    }
    flush(&mut seg, &mut cands, false);
    let Some((mut win, mut gated)) = cands.pop() else {
        return Vec::new();
    };
    if want.is_empty() {
        loop {
            if gated {
                let openers = win.iter().take_while(|l| is_input_opener(l)).count();
                win.drain(..openers);
            }
            if !win.is_empty() && !is_footer_segment(&win) {
                break;
            }
            match cands.pop() {
                Some((prev, prev_rule)) => {
                    win = prev;
                    gated = prev_rule;
                }
                None => return Vec::new(),
            }
        }
    }
    win
}

/// Dialog-specific segmentation: returns (cleaned_winner, raw_winner).
/// In blocked dialogs, '←' does not split headers and options are preserved
/// in raw form so terminal padding rules never drop them before parsing.
pub fn dialog_block(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let mut cands: Vec<(Vec<String>, Vec<String>, bool)> = Vec::new();
    let mut raw_seg: Vec<String> = Vec::new();
    let flush = |raw_seg: &mut Vec<String>,
                 cands: &mut Vec<(Vec<String>, Vec<String>, bool)>,
                 closed_by_rule: bool| {
        let cand = super::filter::dialog_chrome_filtered(raw_seg);
        if !cand.is_empty() {
            cands.push((cand, raw_seg.clone(), closed_by_rule));
        }
        raw_seg.clear();
    };
    for (i, l) in lines.iter().enumerate() {
        let next_non_blank = lines[i + 1..]
            .iter()
            .find(|s| !s.trim().is_empty())
            .map(String::as_str);
        if is_dialog_boundary(l, next_non_blank) {
            flush(&mut raw_seg, &mut cands, is_rule(l));
            continue;
        }
        raw_seg.push(l.clone());
    }
    flush(&mut raw_seg, &mut cands, false);
    let Some((mut cleaned, mut raw, mut gated)) = cands.pop() else {
        return (Vec::new(), Vec::new());
    };
    loop {
        if gated {
            let openers = cleaned.iter().take_while(|l| is_input_opener(l)).count();
            cleaned.drain(..openers);
        }
        if !cleaned.is_empty() && !is_footer_segment(&cleaned) {
            break;
        }
        match cands.pop() {
            Some((prev_clean, prev_raw, prev_rule)) => {
                cleaned = prev_clean;
                raw = prev_raw;
                gated = prev_rule;
            }
            None => return (Vec::new(), Vec::new()),
        }
    }
    (cleaned, raw)
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
