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
/// codex/pi ☰/❯ …, agy ✻/※/⏵ …): after one of these, prior prose is
/// intermediate work.
/// "✱" is the heavy-asterisk tool echo (filter ✻ parity); "~ " is the
/// tilde progress glyph (trailing space so ~/paths never split).
/// "✻" (codex Cooked), "※" (recap) and "⏵" (auto-mode footer) strip as
/// chrome in filter.rs, so they split here too — else old-turn prose
/// merges into the final card.
const TOOL_PREFIXES: &[&str] = &[
    "→", "←", "●", "○", "⏺", "⎿", "☰", "❯", "›", "✱", "~ ", "✻", "※", "⏵",
    "▸ ", "▾", // agy ▸ Thought/Subagents headers + expanded ▾ group
];

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

fn is_continuation_thought(t: &str) -> bool {
    // Bare "Thought"/"Thinking" prefixes ate genuine prose ("Thoughtful
    // review…", "Thinking it over…"): only real TUI headers split —
    // "Thought ·/:/for", sigiled "+/▸ Thought", ellipsis "Thinking…/...".
    t.starts_with("Thought ") || t.starts_with("Thought:") || t.starts_with("Thought for")
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
    if is_continuation_thought(t)
        || t.starts_with("+ Thought")
        || t.starts_with("▸ Thought")
        || t.starts_with("Thinking…")
        || t.starts_with("Thinking...")
        || t.starts_with("Working…")
        || t.starts_with("Working...")
        || t.starts_with("Writing…")
        || t.starts_with("Writing...")
        || t.starts_with("Click to expand")
    {
        return true;
    }
    t.contains('▣') || t.contains("Build ·")
}

pub fn is_dialog_boundary(line: &str, next_line: Option<&str>) -> bool {
    if is_rule(line) {
        // A rule before ANY numbered option keeps the dialog header with
        // its options (first-option-after-rule is the common shape) —
        // splitting there cut the question from its buttons.
        if let Some(next) = next_line
            && is_numbered_option_line(next)
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
    if is_continuation_thought(t)
        || t.starts_with("+ Thought")
        || t.starts_with("▸ Thought")
        || t.starts_with("Thinking…")
        || t.starts_with("Thinking...")
        || t.starts_with("Working…")
        || t.starts_with("Working...")
        || t.starts_with("Writing…")
        || t.starts_with("Writing...")
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
    // Agy processing footer — a lone one never posts as the reply.
    if t.contains("esc to cancel") {
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
    // Agy auto title ("Prioritizing Tool Usage") opens every segment behind
    // a `▸ Thought` header. Drain it at flush — but only when more content
    // follows (a lone line may be the whole reply) and never a fatal
    // provider error (content, not a title). Gated on agy-exact `▸ Thought`:
    // opencode `Thought ·` headers answer directly and drop nothing.
    let mut thought_opened = false;
    let flush = |seg: &mut Vec<String>,
                 cands: &mut Vec<(Vec<String>, bool)>,
                 closed_by_rule: bool,
                 thought_opened: bool| {
        if thought_opened
            && seg.iter().filter(|l| !l.trim().is_empty()).count() >= 2
            && let Some(i) = seg.iter().position(|l| !l.trim().is_empty())
            && !super::notices::is_provider_failure_line(&seg[i])
        {
            seg.remove(i);
        }
        let cand = chrome_filtered(seg);
        if !cand.is_empty() {
            cands.push((cand, closed_by_rule));
        }
        seg.clear();
    };
    for l in lines.iter() {
        if is_boundary(l) {
            flush(&mut seg, &mut cands, is_rule(l), thought_opened);
            echo_pos = None;
            thought_opened = l.trim().starts_with("▸ Thought");
            continue;
        }
        if !want.is_empty() && is_prompt_echo(l, want) {
            flush(&mut seg, &mut cands, false, thought_opened);
            thought_opened = false;
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
    flush(&mut seg, &mut cands, false, thought_opened);
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

#[cfg(test)]
#[path = "segment_bounds_tests.rs"]
mod bounds_tests;
#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
