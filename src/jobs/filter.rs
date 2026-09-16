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
    "─", // turn-separator rule
    "━", // rule
    "═", // rule
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
    "※",
    "• OpenCode",
    "Thought",
    "+ Thought",
    "▸ Thought",
    "Thinking",
    "Working…",
    "Working...",
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
    if is_spinner(line) {
        return true;
    }
    let t = line.trim();
    if t == "…" || t == "..." || t == "⋯" {
        return true;
    }
    // Bare input-box glyphs (empty prompt line) — never content.
    if t == ">" || t == "❯" || t == "›" {
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
pub fn is_dialog_chrome(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with('←') {
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


