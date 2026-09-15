//! Provider-agnostic reply extraction: the fresh reply is the last
//! non-empty output segment after cycle boundaries and prompt echoes.
//!
//! herdr exposes only raw terminal text for every agent; earlier turns and
//! intermediate work are dropped here so cards carry just what was said.

use super::filter::chrome_filtered;

/// Tool-call echo prefixes across providers (opencode →/←, claude ●/⎿,
/// codex/pi ☰/❯ …): after one of these, prior prose is intermediate work.
const TOOL_PREFIXES: &[&str] = &["→", "←", "●", "⎿", "☰", "❯", "›"];

/// A full-width box-rule turn separator (────…): turns are wrapped in these,
/// so the fresh reply follows the last one. ASCII "---" is deliberately NOT
/// a rule — markdown answers use it as content.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 8
        && t.chars()
            .all(|c| c.is_whitespace() || matches!(c, '─' | '━' | '═' | '╍'))
        && t.contains(&['─', '━', '═', '╍'][..])
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

/// The terminal's visible echo of our prompt (framed/`>` input line):
/// everything before it is scrollback from earlier turns. A frame glyph is
/// required so an answer line that merely repeats the prompt is NOT
/// mistaken for an echo.
fn is_prompt_echo(line: &str, want: &str) -> bool {
    if !(line.contains('┃')
        || line.contains('│')
        || line.contains('|')
        || line.trim_start().starts_with('>'))
    {
        return false;
    }
    let t = line
        .trim()
        .trim_start_matches(['┃', '│', '|', '❯', '>', '›', ' ']);
    t == want
}

/// Fresh reply = chrome-cleaned last non-empty segment after cycle
/// boundaries and prompt echoes. A trailing footer/input box yields an
/// empty tail, so the last NON-EMPTY segment wins instead of blanking.
pub fn final_block(lines: &[String], prompt: &str) -> Vec<String> {
    let want = prompt.lines().next().map(str::trim).unwrap_or("");
    let mut seg_start = 0;
    let mut best: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let split = is_boundary(l) || (!want.is_empty() && is_prompt_echo(l, want));
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

    fn agy_screen() -> Vec<String> {
        v(&[
            "Antigravity CLI 1.2.2",
            "  ADC: firebase-adminsdk-fbsvc@ajgc-dig-pdi-dev-cdp",
            "  Gemini 3.8 Flash (High)",
            "  ~/projects/ajnow",
            "────────────────────────────────────────────────",
            "> hi",
            "  Hello! How can I help you today?",
            "────────────────────────────────────────────────",
            "> 2-4?",
            "  -2 (or the range from 2 to 4, depending on",
            "  context).",
            "───────────────────────────────────────────────────",
            ">",
            "───────────────────────────────────────────────────",
            "? for shortcuts             Gemini 3.8 Flash · high",
        ])
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
            "   /home/user/projects/herdr-telegram        10.8K (1%)  ctrl+p commands",
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

    #[test]
    fn test_agy_spontaneous_keeps_last_turn_only() {
        // No prompt known: banner, old turns, rules, input box and footer
        // all drop — only the last turn (echo + answer) survives.
        assert_eq!(
            final_block(&agy_screen(), ""),
            v(&[
                "> 2-4?",
                "  -2 (or the range from 2 to 4, depending on",
                "  context).",
            ])
        );
    }

    #[test]
    fn test_agy_prompted_drops_echo_too() {
        // Prompt known: the `>` echo line splits as well — answer only.
        assert_eq!(
            final_block(&agy_screen(), "2-4?"),
            v(&[
                "  -2 (or the range from 2 to 4, depending on",
                "  context).",
            ])
        );
    }

    #[test]
    fn test_ascii_rule_and_quotes_never_split() {
        // Markdown `---`, blockquotes, diff and table lines are content.
        let lines = v(&["intro", "---", "> quoted", "> added", "| a | b |", "end"]);
        assert_eq!(final_block(&lines, "q"), lines);
    }

    #[test]
    fn test_permission_dialog_keeps_question_and_options() {
        // Real opencode permission dialog as herdr reports it: everything
        // framed. De-framed, path + options survive the pipeline while
        // footers drop (see waiting_text).
        use crate::jobs::filter::deframe;
        let raw = v(&[
            "  ┃",
            "  ┃  △ Permission required",
            "  ┃    ← Access external directory ~/.config/opencode",
            "  ┃",
            "  ┃  Patterns",
            "  ┃",
            "  ┃  - /home/user/.config/opencode/*",
            "  ┃",
            "  ┃",
            "  ┃   Allow once   Allow always   Reject",
            "  ┃",
            "  ┃  ctrl+f fullscreen  ⇆ select  enter confirm",
            "  ┃",
        ]);
        assert_eq!(
            deframe("  ┃  △ Permission required"),
            "△ Permission required"
        );
        assert_eq!(deframe("plain line"), "plain line");
        let lines: Vec<String> = raw.iter().map(|l| deframe(l)).collect();
        let out = final_block(&lines, "");
        let body = out.join("\n");
        assert!(body.contains("Allow once"), "options kept: {body}");
        assert!(body.contains("opencode/*"), "path kept: {body}");
        assert!(!body.contains("ctrl+p"), "footer dropped: {body}");
    }
}
