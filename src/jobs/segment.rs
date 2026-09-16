//! Provider-agnostic reply extraction: the fresh reply is the last
//! non-empty output segment after cycle boundaries and prompt echoes.
//!
//! herdr exposes only raw terminal text for every agent; earlier turns and
//! intermediate work are dropped here so cards carry just what was said.

use super::{
    echo::{echo_rest, is_echo_continuation, is_input_opener, is_prompt_echo},
    filter::chrome_filtered,
};

/// Tool-call echo prefixes across providers (opencode →/←, claude ●/⎿,
/// codex/pi ☰/❯ …): after one of these, prior prose is intermediate work.
const TOOL_PREFIXES: &[&str] = &["→", "←", "●", "○", "⎿", "☰", "❯", "›"];

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

/// Fresh reply = chrome-cleaned last non-empty segment after cycle
/// boundaries and prompt echoes. A trailing footer/input box yields an
/// empty tail, so the last NON-EMPTY segment wins instead of blanking.
///
/// Two echo hazards, both agy-shaped. Prompted path: only the echo opener
/// carries framing, so a multi-line prompt's wrap lines would outlive the
/// opener split and shadow the real answer above — they are skipped as an
/// echo region instead (opener + prompt-matching wraps, see echo.rs).
/// Spontaneous path (no prompt to match): the settled screen always ends
/// on the input box, so leading `>` lines of a RULE-terminated winning
/// segment are the box, never the reply — strip them and fall back to the
/// previous segment. The rule gate matters: a `>`-led question with no
/// closing rule (blocked dialogs, quote excerpts) is content and stays.
/// (Residual: wraps of a multi-line terminal-typed prompt are
/// indistinguishable from indented prose without the prompt text — only
/// the `>` opener strips there.)
pub fn final_block(lines: &[String], prompt: &str) -> Vec<String> {
    let want = prompt.lines().next().map(str::trim).unwrap_or("");
    let rest = echo_rest(prompt);
    // (segment, closed_by_rule): the strip gate needs each candidate's
    // terminator, so history keeps it alongside.
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
            // Blank echo lines ride along without advancing the chain
            // (multi-line prompts echo their blank lines too).
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
    // The prompted path already split its echo above; stripping there
    // would eat blockquote-led answers, so it stays untouched.
    if want.is_empty() {
        loop {
            if gated {
                let openers = win.iter().take_while(|l| is_input_opener(l)).count();
                win.drain(..openers);
            }
            if !win.is_empty() {
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
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
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

    #[test]
    fn test_final_block_drops_agy_tool_noise() {
        let lines = v(&[
            "> build the project",
            "▸ Thought for 11s, 1.5k tokens",
            "● Edit(src/main.rs)",
            "○ Bash(cargo test)",
            "⡿ Running command...",
            "└ Tip: Use --release for faster builds",
            "────────────────────────────────────────────────",
            ">",
            "? for shortcuts             Gemini 3.8 Flash · high",
        ]);
        assert_eq!(
            final_block(&lines, "build the project"),
            Vec::<String>::new()
        );

        let lines_with_answer = v(&[
            "> build the project",
            "▸ Thought for 11s, 1.5k tokens",
            "● Edit(src/main.rs)",
            "○ Bash(cargo test)",
            "     Build succeeded with 0 errors.",
            "────────────────────────────────────────────────",
            ">",
        ]);
        assert_eq!(
            final_block(&lines_with_answer, "build the project"),
            v(&["     Build succeeded with 0 errors."])
        );

        let lines_thinking = v(&["> build the project", "▸ Thought for 11s, 1.5k tokens"]);
        assert_eq!(
            final_block(&lines_thinking, "build the project"),
            Vec::<String>::new()
        );
    }
}
