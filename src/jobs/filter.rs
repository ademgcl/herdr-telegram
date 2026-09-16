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
    "⎿",
    "☰",
    "❯",
    "›",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chrome_strips_opencode_tui() {
        assert!(is_chrome("┃  [forum] topic msg for wG:p1: hii"));
        assert!(is_chrome("   Thought · 359ms"));
        assert!(is_chrome("+ Thought: 6.8s"));
        assert!(is_chrome("  ▣  Build · Muse Spark 1.3 Free · 1m 3s"));
        assert!(is_chrome("  Plan · Ling · Zen · high"));
        assert!(is_chrome("╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀"));
        assert!(is_chrome("  ⠋ working…"));
        assert!(is_chrome("→Read src/telegram/router.rs"));
        assert!(is_chrome("← Edit src/jobs/reply.rs"));
        assert!(is_chrome("[watcher] start wG:p1"));
        assert!(is_chrome("[✓] Add session_id to AgentDetail"));
        assert!(is_chrome("# Todos"));
        assert!(is_chrome("Click to expand"));
        assert!(!is_chrome("hello there, how can I help?"));
    }

    #[test]
    fn test_chrome_keeps_prose() {
        // Bare provider words / key hints in normal answers must survive.
        assert!(!is_chrome("refresh tokens expire after an hour"));
        assert!(!is_chrome("I spent an hour on this"));
        assert!(!is_chrome("OpenCode is great for this"));
        assert!(!is_chrome("press ctrl+c to stop"));
        assert!(!is_chrome("Hi! I'm Muse Spark. How can I help today?"));
        // Short indented lines are answers, not padding.
        assert!(!is_chrome("     hi"));
        assert!(!is_chrome("     2"));
        assert!(!is_chrome("     ok"));
        // …but long whitespace-padded art still goes.
        assert!(is_chrome("                              *  *  *"));
    }

    #[test]
    fn test_chrome_strips_agy_tui() {
        assert!(is_chrome(
            "────────────────────────────────────────────────"
        ));
        assert!(is_chrome(">"));
        assert!(is_chrome(
            "? for shortcuts             Gemini 3.8 Flash · high"
        ));
        assert!(is_chrome("Antigravity CLI 1.2.2"));
        assert!(is_chrome(
            "  ADC: firebase-adminsdk-fbsvc@ajgc-dig-pdi-dev-cdp"
        ));
        assert!(is_chrome("○ Bash(cargo test)"));
        assert!(is_chrome("⡿ Running command..."));
        assert!(is_chrome("└ Tip: Run with --nocapture"));
        assert!(is_chrome("▸ Thought for 11s, 1.5k tokens"));
        assert!(is_chrome("  ▸ Thought for 4s"));
        // …but quoted/diff/table content and ASCII rules survive.
        assert!(!is_chrome("> quoted text"));
        assert!(!is_chrome("> added line"));
        assert!(!is_chrome("| a | b |"));
        assert!(!is_chrome("intro --- still content"));
        assert!(!is_chrome("here is a tip for your code"));
        // …and prose mentioning the CLI inline survives (anchored only).
        assert!(!is_chrome("I use Antigravity CLI daily"));
        assert!(!is_chrome("the ADC value rose today"));
    }

    #[test]
    fn test_chrome_keeps_fatal_provider_error_even_framed() {
        let err = "Error from provider (Console): Upstream request failed: [invalid_request_error] reasoning `encrypted_content` was not issued to this caller";
        assert!(!is_chrome(err));
        assert!(!is_chrome(&format!("  ┃  {err}")));
        assert!(!chrome_filtered(&[format!("  ┃  {err}")]).is_empty());
    }
}
