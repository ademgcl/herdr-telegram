//! Prompt-echo recognition for reply extraction (see `segment`).
//!
//! Terminal agents echo our submitted prompt back into the scrollback
//! (opencode's framed `┃ > hi`, agy's bare `> hi`). Echo lines are input,
//! never answer content — but only the FIRST echo line carries framing,
//! so a multi-line prompt needs chain matching: the opener must equal
//! prompt line 1, and each following echo line must equal the NEXT prompt
//! line in order. `segment` runs the chain, so a coincidental answer match
//! outside an echo region can never trigger it.

/// Trimmed non-empty prompt lines after the first — the echo-continuation
/// vocabulary. Empty for single-line prompts (the chain never engages).
pub fn echo_rest(prompt: &str) -> Vec<&str> {
    prompt
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// Framing providers wrap input echoes in (opencode `┃`/`│`, agy `>`/`❯`/`›`,
/// pi/codex `|`). Continuations compare by content, not chrome.
fn strip_echo_framing(line: &str) -> &str {
    line.trim()
        .trim_start_matches(['┃', '│', '|', '❯', '>', '›', ' '])
}

/// The terminal's visible echo of our prompt (framed/`>` input line):
/// everything before it is scrollback from earlier turns. A frame glyph is
/// required so an answer line that merely repeats the prompt is NOT
/// mistaken for an echo.
pub fn is_prompt_echo(line: &str, want: &str) -> bool {
    let t = line.trim_start();
    if !(t.starts_with('┃') || t.starts_with('│') || t.starts_with('|') || t.starts_with('>')) {
        return false;
    }
    strip_echo_framing(line) == want
}

/// A wrapped line of a multi-line prompt echo, matched IN ORDER: after
/// the opener (prompt line 1), each echo line must equal the next prompt
/// line, `pos` counting those already consumed. Accepts framed and bare
/// wraps — agy wraps long input with a plain indent, opencode re-frames
/// with `┃`. Order matters: an answer line that merely equals some LATER
/// prompt line out of order is content, never echo.
pub fn is_echo_continuation(line: &str, rest: &[&str], pos: usize) -> bool {
    let Some(want) = rest.get(pos) else {
        return false;
    };
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    t == *want || strip_echo_framing(line) == *want
}

/// A segment opener that is an input box, not an answer: agy's bare `>`
/// box (opencode's `❯`/`›` boxes already split as boundaries, so only
/// bare `>` can open a segment). Spontaneous extraction has no prompt to
/// match against, so a trailing box is recognized by position instead.
pub fn is_input_opener(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::segment::final_block;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_echo_rest_skips_first_and_blanks() {
        assert!(echo_rest("solo").is_empty());
        assert_eq!(echo_rest("one\ntwo\n\n  three  "), vec!["two", "three"]);
    }

    #[test]
    fn test_echo_continuation_in_order_only() {
        let rest = ["second line", "third"];
        assert!(is_echo_continuation("  second line", &rest, 0));
        assert!(!is_echo_continuation("  third", &rest, 0));
        assert!(is_echo_continuation("  ┃  third", &rest, 1));
        assert!(!is_echo_continuation("  unrelated", &rest, 0));
        assert!(!is_echo_continuation("  second line", &rest, 2));
        assert!(!is_echo_continuation("   ", &rest, 0));
    }

    #[test]
    fn test_prompt_echo_needs_framing() {
        // An answer line repeating the prompt is content, not an echo.
        assert!(is_prompt_echo("  ┃  hi", "hi"));
        assert!(is_prompt_echo("> hi", "hi"));
        assert!(!is_prompt_echo("     hi", "hi"));
        // A pipe/table char buried mid-line is content, not framing.
        assert!(!is_prompt_echo("a | b", "a | b"));
        assert!(!is_prompt_echo("col ┃ val", "col ┃ val"));
    }

    #[test]
    fn test_input_opener_is_bare_box_only() {
        assert!(is_input_opener("> go"));
        assert!(!is_input_opener("  plain answer"));
        // `❯`/`›` boxes split as boundaries, so they never open segments.
        assert!(!is_input_opener("❯ go"));
    }

    fn agy_screen() -> Vec<String> {
        v(&[
            "Antigravity CLI 1.2.2",
            "  ADC: firebase-adminsdk-fbsvc@test-proj-123",
            "  Gemini 3.8 Flash (High)",
            "  ~/projects/demo",
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
    fn test_agy_spontaneous_strips_input_box_keeps_answer() {
        // No prompt known: banner, old turns, rules, input box and footer
        // all drop — only the last turn's ANSWER survives (the `>` echo
        // opener is input, never output).
        assert_eq!(
            final_block(&agy_screen(), ""),
            v(&[
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
    fn test_agy_prompted_multiline_echo_region_dropped() {
        // Live shape: a two-line prompt echoes as opener + bare wrap, and
        // the wrap used to outlive the opener split and shadow the real
        // answer above as the "fresh reply".
        let screen = v(&[
            "  prior answer prose that took real work",
            "───────────────────────────────────────────────────",
            "> http://localhost:5175/events/evt_199523ae local doesnt work",
            "  this e.g. show",
            "───────────────────────────────────────────────────",
            "                                          Gemini 3.8 Flash · high",
        ]);
        let prompt = "http://localhost:5175/events/evt_199523ae local doesnt work\nthis e.g. show";
        assert_eq!(
            final_block(&screen, prompt),
            v(&["  prior answer prose that took real work"])
        );
    }

    #[test]
    fn test_echo_region_ends_at_first_non_match() {
        // The chain eats echo wraps only: the first line that is neither
        // boundary nor the next prompt line ends the region and stays.
        let screen = v(&[
            "  older answer",
            "──────────────────────────────────",
            "> do it",
            "  extra wrapped prompt line",
            "  fresh answer after the box",
            "──────────────────────────────────",
            "                                          Gemini 3.8 Flash · high",
        ]);
        assert_eq!(
            final_block(&screen, "do it\nextra wrapped prompt line"),
            v(&["  fresh answer after the box"])
        );
    }

    #[test]
    fn test_echo_chain_skips_blank_lines() {
        // Multi-line prompts echo their blank lines too: blanks ride
        // along without breaking the chain or advancing it.
        let screen = v(&[
            "  real answer",
            "──────────────────────────────────",
            "> do it",
            "",
            "  push it",
            "──────────────────────────────────",
            "                                          Gemini 3.8 Flash · high",
        ]);
        assert_eq!(
            final_block(&screen, "do it\n\npush it"),
            v(&["  real answer"])
        );
    }

    #[test]
    fn test_echo_chain_is_order_sensitive() {
        // An answer line equal to a LATER prompt line out of order is
        // content: the chain only accepts the next expected line.
        let screen = v(&[
            "──────────────────────────────────",
            "> go",
            "  third",
            "  genuine answer",
            "──────────────────────────────────",
            "                                          Gemini 3.8 Flash · high",
        ]);
        assert_eq!(
            final_block(&screen, "go\nsecond\nthird"),
            v(&["  third", "  genuine answer"])
        );
    }

    #[test]
    fn test_framed_multiline_wrap_skipped() {
        // Opencode re-frames wrapped echo lines with `┃`: content match
        // ignores the framing.
        let screen = v(&["  ┃  fix bug", "  ┃  in auth", "     Fixed."]);
        assert_eq!(
            final_block(&screen, "fix bug\nin auth"),
            v(&["     Fixed."])
        );
    }

    #[test]
    fn test_unterminated_quote_segment_kept() {
        // A `>`-led segment with NO closing rule is content (blocked
        // dialog questions, quote excerpts) — only rule-terminated boxes
        // strip on the spontaneous path.
        let screen = v(&[
            "△ Permission required",
            "> quoted excerpt from the task",
            "Allow once   Allow always   Reject",
        ]);
        assert_eq!(final_block(&screen, ""), screen);
    }

    #[test]
    fn test_spontaneous_box_only_turn_falls_back_to_last_answer() {
        // Prompt echoed, agent answered nothing yet: the trailing box
        // strips to nothing, so the last real answer wins instead of the
        // prompt echoing back as the "fresh reply".
        let screen = v(&[
            "  older answer",
            "──────────────────────────────────",
            "> pending question with no reply yet",
            "──────────────────────────────────",
            "                                          Gemini 3.8 Flash · high",
        ]);
        assert_eq!(final_block(&screen, ""), v(&["  older answer"]));
    }
}
