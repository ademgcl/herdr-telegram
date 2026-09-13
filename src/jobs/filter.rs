/// Provider-agnostic TUI output filtering and reply extraction.
///
/// herdr exposes only raw terminal text for every agent, so the bot distills
/// the fresh reply out of frame chrome, tool echoes and earlier turns here.

/// Substrings that only appear in TUI footers, status bars and frame
/// chrome across providers. Kept narrow (key-hint combos, middle-dot
/// headers, frame glyphs): plain prose almost never contains these, so
/// real answers survive. NOTE: bare words like "tokens"/"spent" or provider
/// names are deliberately NOT here — they occur in normal answers.
const CHROME_MARKERS: &[&str] = &[
    "ctrl+p",
    "esc interrupt",
    "Thought ·",
    "Build ·",
    "Click to expand",
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
];

/// Start-anchored prefixes for tool-call echoes, reasoning headers,
/// collapsed sections and herdr/bot log lines. Anchored (not contains) so
/// markdown prose, code and lists survive.
const CHROME_PREFIXES: &[&str] = &[
    "→", "←", "●", "⎿", "☰", "❯", "›",
    "Thought", "+ Thought",
    "Thinking", "Working…", "Working...",
    "Click to expand",
    "Todos", "# Todos",
    "[✓]", "[•]", "[x]", "[X]", "[ ]",
    "[herdr]", "[tg]", "[forum]", "[watcher]", "[prompt]", "[events]", "[alert]",
];

/// Tool-call echo prefixes across providers (opencode →/←, claude ●/⎿,
/// codex/pi ☰/❯ …): after one of these, prior prose is intermediate work.
const TOOL_PREFIXES: &[&str] = &["→", "←", "●", "⎿", "☰", "❯", "›"];

/// Spinner braille frames (⠋⠙⠹…) — pure progress, never content.
fn is_spinner(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    t.chars()
        .all(|c| matches!(c, '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏'))
}

/// A line of pure interface (spinner frames, footers, borders, tool
/// echoes, reasoning headers, log lines, blank padding).
pub fn is_chrome(line: &str) -> bool {
    if is_spinner(line) {
        return true;
    }
    let t = line.trim();
    if t == "…" || t == "..." || t == "⋯" {
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

/// A line that opens a new output cycle: tool-call echoes, reasoning
/// headers, step footers. The agent's fresh reply is whatever follows the
/// LAST one — earlier segments are intermediate work, not the answer.
/// (Deliberately narrow: todo/log lines can appear inside tool output, so
/// they never split.)
pub fn is_boundary(line: &str) -> bool {
    let t = line.trim();
    if TOOL_PREFIXES.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    if t.starts_with("Thought")
        || t.starts_with("+ Thought")
        || t.starts_with("Thinking")
        || t.starts_with("Working…")
        || t.starts_with("Working...")
        || t.starts_with("Click to expand")
    {
        return true;
    }
    t.contains('▣') || t.contains("Build ·")
}

/// The terminal's visible echo of our prompt (framed input line):
/// everything before it is scrollback from earlier turns. A frame glyph is
/// required so an answer line that merely repeats the prompt is NOT
/// mistaken for an echo.
fn is_prompt_echo(line: &str, want: &str) -> bool {
    if !(line.contains('┃') || line.contains('│') || line.contains('|')) {
        return false;
    }
    let t = line
        .trim()
        .trim_start_matches(|c: char| matches!(c, '┃' | '│' | '|' | '❯' | '>' | '›' | ' '));
    t == want
}

/// Fresh reply = chrome-cleaned tail after the last cycle boundary or
/// prompt echo. Earlier turns and intermediate work are dropped, so the
/// card carries only what the agent just said — for any provider.
/// A trailing footer boundary (idle status bar) yields an empty tail, so
/// the last NON-EMPTY segment wins instead of blanking the answer.
pub fn final_block(lines: &[String], prompt: &str) -> Vec<String> {
    let want = prompt.lines().next().map(str::trim).unwrap_or("");
    let mut seg_start = 0;
    let mut best: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let split =
            is_boundary(l) || (!want.is_empty() && is_prompt_echo(l, want));
        if split {
            let cand = chrome_filtered(&lines[seg_start..i]);
            if !cand.is_empty() {
                best = cand;
            }
            seg_start = i + 1;
        }
    }
    let tail = chrome_filtered(&lines[seg_start..]);
    if !tail.is_empty() { tail } else { best }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_chrome_strips_opencode_tui() {
        assert!(is_chrome("┃  [forum] topic msg for wG:p1: hii"));
        assert!(is_chrome("   Thought · 359ms"));
        assert!(is_chrome("+ Thought: 6.8s"));
        assert!(is_chrome("  ▣  Build · Muse Spark 1.3 Free · 1m 3s"));
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
    fn test_idle_screen_yields_answer_not_footer() {
        // A settled screen (frame + Thought header + answer + step footer
        // + idle footer) reduces to the answer — never status-bar soup.
        let screen = v(&[
            "  ┃",
            "  ┃  hi",
            "  ┃",
            "     Thought · 539ms",
            "     Hi! I'm Muse Spark. How can I help with herdr-telegram today?",
            "     ▣  Build · Muse Spark 1.3 Free · 5.1s",
            "  ┃",
            "  ┃  Build · Muse Spark 1.3 Free OpenCode Zen · xhigh",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
            "   /Users/adem/projects/herdr-telegram        10.8K (1%)  ctrl+p commands",
        ]);
        assert_eq!(
            final_block(&screen, "hi"),
            v(&["     Hi! I'm Muse Spark. How can I help with herdr-telegram today?"])
        );
    }

    #[test]
    fn test_final_block_drops_work_before_tool_call() {
        let lines = v(&[
            "     I'll take a look first.",
            "→Read src/main.rs",
            "     Found it — the guard is missing.",
        ]);
        assert_eq!(
            final_block(&lines, "fix it"),
            v(&["     Found it — the guard is missing."])
        );
    }

    #[test]
    fn test_final_block_splits_multi_turn_scrollback() {
        // Earlier turns above the prompt echo are dropped; only the fresh
        // reply survives.
        let lines = v(&[
            "     Hi — this is herdr-telegram.",
            "     You've got 4 modified files.",
            "  ┃",
            "  ┃  hi",
            "  ┃",
            "     Hey — what do you need in herdr-telegram?",
        ]);
        assert_eq!(
            final_block(&lines, "hi"),
            v(&["     Hey — what do you need in herdr-telegram?"])
        );
    }

    #[test]
    fn test_final_block_keeps_unframed_prompt_repeat() {
        // An answer line repeating the prompt is content, not an echo:
        // only the framed echo splits.
        let lines = v(&["  ┃  hi", "     hi", "     How can I help?"]);
        assert_eq!(
            final_block(&lines, "hi"),
            v(&["     hi", "     How can I help?"])
        );
    }

    #[test]
    fn test_final_block_no_boundaries_returns_all() {
        let lines = v(&["first line", "second line"]);
        assert_eq!(final_block(&lines, "hi"), lines);
    }
}
