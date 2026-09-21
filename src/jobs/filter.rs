//! Provider-agnostic TUI chrome filtering.
//!
//! herdr exposes only raw terminal text for every agent; frame chrome,
//! footers and progress noise are dropped here (see segment.rs for reply
//! extraction out of multi-turn scrollback).

/// Substrings that only appear in TUI footers, status bars and frame
/// chrome across providers. Kept narrow (key-hint combos, middle-dot
/// headers, frame/rule glyphs, footer hints): plain prose almost never
/// contains these, so real answers survive. NOTE: bare words like
/// "tokens"/"spent" or provider names are deliberately NOT here — they
/// occur in normal answers.
const CHROME_MARKERS: &[&str] = &[
    "ctrl+p",
    "esc interrupt",
    "esc to cancel", // agy processing footer (Claude's ⏵ auto-mode twin)
    "Thought ·",
    "Build ·",
    "Plan ·",
    "Click to expand",
    "? for shortcuts",
    "% used",
    "◔",
    "███",
    "┃", // TUI frame
    "╹", // frame
    "╻", // frame
    "▣", // status card
    "⬝", // bullet
    "⠋", // spinner
    "⠙", // spinner
    "⠹", // spinner
    "⠸", // spinner
    "⠼", // spinner
    "⠴", // spinner
    "⠦", // spinner
    "⠧", // spinner
    "⠇", // spinner
    "⠏", // spinner
    "⡿", // spinner
    "⣟", // spinner
    "⣯", // spinner
    "⣷", // spinner
    "⣾", // spinner
    "⣽", // spinner
    "⣻", // spinner
    "⢿", // spinner
    "└ Tip:",
    "auto mode on",
    "shift+tab to cycle",
    "Enter to select",
    "enter to select",
    "Tab/Arrow",
    "tab/arrow",
    "ctrl+f fullscreen",
    "view transcript",
];

/// Start-anchored prefixes for tool-call echoes, reasoning headers,
/// collapsed sections and herdr/bot log lines. Anchored (not contains) so
/// markdown prose, code and lists survive. Accepted trade: an answer line
/// STARTING with one of these (e.g. a reply that itself begins
/// "Antigravity CLI …") strips — vanishingly rare vs header noise on
/// every agy turn, and the arbitration merge still recovers the rest.
/// "✱" is the heavy-asterisk tool echo (✻ parity); "~ " is the tilde
/// progress glyph (trailing space so ~/paths survive); "Writing…"
/// is the progress verb (Working… parity).
const CHROME_PREFIXES: &[&str] = &[
    "→",
    "←",
    "●",
    "○",
    "⏺",
    "⎿",
    "☰",
    "❯",
    "›",
    "⏵",
    "✻",
    "✱",
    "~ ",
    "※",
    "• OpenCode",
    // Narrow Thought/Thinking headers (segment parity): bare prefixes
    // ate genuine prose ("Thoughtful review…", "Thinking it over…").
    // "Thought " covers "Thought ·" + "Thought for"; "Thinking" only
    // ever ships with an ellipsis in the TUI.
    "Thought ",
    "Thought:",
    "+ Thought",
    "▸ Thought",
    "▸ ", // agy ▸ Subagents group headers (▸ Thought covered too)
    "▾", // agy expanded ▾ Subagents group header
    "Thinking…",
    "Thinking...",
    "Working…",
    "Working...",
    "Writing…",
    "Writing...",
    "Click to expand",
    "Todos",
    "# Todos",
    "Antigravity CLI",
    "ADC:",
    "[✓]",
    "[•]",
    "[x]",
    "[X]",
    "[ ]",
    "[herdr]",
    "[tg]",
    "[forum]",
    "[watcher]",
    "[prompt]",
    "[events]",
    "[alert]",
];

/// Spinner braille frames (⠋⠙⠹… / ⡿⣟⣯…) — pure progress, never content.
fn is_spinner(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    t.chars().all(|c| {
        matches!(
            c,
            '⠋' | '⠙'
                | '⠹'
                | '⠸'
                | '⠼'
                | '⠴'
                | '⠦'
                | '⠧'
                | '⠇'
                | '⠏'
                | '⡿'
                | '⣟'
                | '⣯'
                | '⣷'
                | '⣾'
                | '⣽'
                | '⣻'
                | '⢿'
        )
    })
}

/// A line of pure interface (spinner frames, footers, borders, tool
/// echoes, reasoning headers, log lines, blank padding).
pub fn is_chrome(line: &str) -> bool {
    // Fatal provider errors are content, never chrome — even when the
    // TUI frames them with ┃/│ rails. Dropping them here blanked the
    // tail and let stale turns resurface as the "answer".
    if super::notices::is_provider_failure_line(line) {
        return false;
    }
    // Full-width box rules are chrome only as a full rule (single
    // source: segment::is_rule — a lone ─ in prose like `foo ─ bar`
    // is content, never chrome).
    if super::segment::is_rule(line) {
        return true;
    }
    if is_spinner(line) {
        return true;
    }
    let t = line.trim();
    if t == "…" || t == "..." || t == "⋯" {
        return true;
    }
    // Bare input-box glyphs (empty prompt line) — never content.
    // "~" is the bare progress glyph ("~ Writing…" lines strip via the
    // "~ " prefix above; the lone glyph is never a reply).
    if t == ">" || t == "❯" || t == "›" || t == "~" {
        return true;
    }
    if CHROME_PREFIXES.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    if CHROME_MARKERS.iter().any(|m| line.contains(m)) {
        return true;
    }
    // Blank padding is chrome. The whitespace-ratio rule below only applies
    // to LONG lines: TUI content is indented, so short lines ("hi", "ok",
    // "2") must survive — only sweeping art/status padding is dropped.
    let chars = line.chars().count();
    let visible = line.chars().filter(|c| !c.is_whitespace()).count();
    visible == 0 || (chars >= 30 && visible * 3 < chars)
}

/// Drop TUI chrome (spinners, footers, borders) so streams carry meaning.
pub fn chrome_filtered(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.as_str())
        .filter(|l| !is_chrome(l))
        .map(|l| l.to_string())
        .collect()
}

/// Strip one level of TUI frame rails (┃│|) for content recovery: dialog
/// text and option rows render framed, and de-framed they go through the
/// normal boundary/chrome pipeline. Only used for blocked-card question
/// extraction — NOT the live stream (there, frames usefully mark noise).
pub fn deframe(line: &str) -> String {
    let mut t = line.trim();
    while let Some(rest) = t.strip_prefix(|c: char| matches!(c, '┃' | '│' | '|')) {
        t = rest.trim_start();
    }
    t.to_string()
}

/// In blocked dialogs, '←' heads the requested action or header (← Access ...,
/// ← ☐ Partial ...), never tool echoes, so it survives as content.
/// Numbered ❯ options (❯ 1. foo) survive too — segment keeps them in the
/// same dialog block, so filtering them here would empty the question.
pub fn is_dialog_chrome(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with('←') {
        return false;
    }
    if super::segment::is_numbered_option_line(line) {
        return false;
    }
    is_chrome(line)
}

/// Chrome filtering for blocked dialog questions: preserves header arrows.
pub fn dialog_chrome_filtered(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.as_str())
        .filter(|l| !is_dialog_chrome(l))
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
