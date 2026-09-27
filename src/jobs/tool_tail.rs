//! Cut a turn's tool block out of a reply body.
//!
//! Why this exists: an agent TUI interleaves the assistant's prose with
//! the tool calls it runs, and the tool call is usually the LAST thing
//! on screen. A body taken from "the last block" is therefore the
//! command the agent just ran plus its output — which is what users
//! were getting: their card showing `$ herdr pane read …` and a python
//! heredoc instead of the answer.
//!
//! The shape is structural, not semantic, which is the point. Measured
//! off a live Kilo pane:
//!
//! ```text
//!  5 |  Fair challenge. To be straight: I ran the CLI…   ← prose
//!  5 |  $ herdr pane read wZ:p1 > /tmp/real2.txt        ← tool call
//!  7 |    python3 - <<'PY'                               ← tool output
//! 11 |      m=re.search(…)                               ← nested output
//! ```
//!
//! Prose sits at the block's own minimum indent; a tool call starts on
//! that same indent behind a command marker, and its output is
//! indented deeper. So a tool block runs from a marked line at prose
//! indent while the following lines stay deeper — and ENDS the moment
//! prose returns to prose indent, which is exactly the case of an
//! agent that writes its answer after running tools.
//!
//! No word shapes, no vocabulary, no "does this look like prose". A
//! one-line reply cannot be mistaken for a command, and a status bar
//! cannot be mistaken for an answer. That is why it does not have the
//! failure modes of the three prose heuristics this replaced.

/// A tool-call marker on a line: a shell prompt, or a script opener.
/// Deliberately narrow — a line merely *containing* `$` is prose.
pub fn is_tool_call(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("$ ") {
        return !rest.trim().is_empty();
    }
    // Heredoc openers run to end-of-line, so the call wraps onto the
    // next line: `python3 - <<'PY'` / `cat <<EOF`. The delimiter must
    // follow `<<` immediately — `a << b` is prose, not a heredoc.
    t.match_indices("<<").any(|(i, _)| {
        t[i + 2..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && c != '<')
    })
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// Remove the trailing tool block, keeping prose that follows it.
pub fn cut_tool_tail(lines: &[String]) -> Vec<String> {
    let prose_at = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);
    let start = lines
        .iter()
        .position(|l| !l.trim().is_empty() && indent(l) == prose_at && is_tool_call(l));
    let Some(start) = start else {
        return lines.to_vec();
    };
    // Walk past the call and everything deeper than prose — that is the
    // output. Stop at the first non-empty line back at prose indent:
    // the agent wrote prose after the tool, and it must survive.
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if !l.trim().is_empty() && indent(l) <= prose_at && !is_tool_call(l) {
            end = i;
            break;
        }
    }
    let mut out: Vec<String> = lines[..start].to_vec();
    out.extend(lines[end..].iter().cloned());
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{cut_tool_tail, is_tool_call};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    static N: AtomicU64 = AtomicU64::new(0);

    /// The live pane, verbatim (indents kept). This is the case that
    /// shipped broken cards: the body was the command, not the answer.
    #[test]
    fn test_live_pane_tool_block_is_cut() {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let lines = v(&[
            "     Fair challenge. I ran the CLI on fixtures.",
            "     $ herdr pane read wZ:p1 > /tmp/real2.txt",
            "       python3 - <<'PY'",
            "       import re",
            "           m=re.search(r' {20,}', l)",
            "       PY",
            "     Done.",
        ]);
        let _ = n;
        let out = cut_tool_tail(&lines);
        let text = out.join("\n");
        assert!(text.contains("Fair challenge"), "prose lost: {text:?}");
        assert!(
            text.contains("Done."),
            "prose after the tool lost: {text:?}"
        );
        for junk in ["herdr pane read", "python3", "import re", "re.search", "PY"] {
            assert!(
                !text.contains(junk),
                "tool output leaked: {junk:?} in {text:?}"
            );
        }
    }

    /// No tool call: the body must be untouched, byte for byte.
    #[test]
    fn test_a_pure_reply_is_untouched() {
        let lines = v(&["     Short answer.", "     Second line."]);
        assert_eq!(cut_tool_tail(&lines), lines);
    }

    /// A one-line reply cannot be mistaken for a command, whatever it
    /// contains. This is the case the prose heuristics got wrong.
    #[test]
    fn test_short_reply_is_never_a_command() {
        for s in [
            "     ok",
            "     Yes",
            "     It costs $5 and $10 total",
            "     The fix is in src/jobs/report.rs",
        ] {
            let lines = v(&[s]);
            assert_eq!(cut_tool_tail(&lines), lines, "altered: {s:?}");
        }
    }

    /// A `<<` inside ordinary prose is not a heredoc opener.
    #[test]
    fn test_heredoc_marker_is_narrow() {
        assert!(is_tool_call("$ cargo test"));
        assert!(is_tool_call("  cat <<EOF"));
        assert!(!is_tool_call("The a << b comparison holds"));
        assert!(!is_tool_call("$"));
    }

    /// Two tool calls in one turn: both are cut.
    #[test]
    fn test_two_tool_calls_are_both_cut() {
        let lines = v(&[
            "     Answer first.",
            "     $ cargo test",
            "       compiling...",
            "     $ cargo clippy",
            "       warning: unused",
            "     Answer last.",
        ]);
        let text = cut_tool_tail(&lines).join("\n");
        assert!(text.contains("Answer first.") && text.contains("Answer last."));
        assert!(!text.contains("cargo") && !text.contains("warning"));
    }
}
