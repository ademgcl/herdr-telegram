//! Column-cut regressions for SINGLE-COLUMN agents (split from
//! `screens`: 500-line file limit).
//!
/// The column cut exists for a two-column TUI (Kilo). Every other
/// supported agent renders ONE column, and this battery proves the cut
/// does not touch them.
///
/// These are not invented: the chrome shapes are the shared regression
/// inventory's verbatim captures (opencode's 200-char status bar, agy's
/// frames, claude/codex rules), and the reply bodies are its PROSE
/// entries, which include aligned tables and indented code — the shapes
/// most likely to fake a column split. A misfire here truncates a real
/// answer for 17 of the 18 supported agents.
#[cfg(test)]
mod t {
    use super::super::strip_columns;

    fn screen(lines: &[String]) -> String {
        lines.join("\n")
    }

    /// A wide, aligned body: code columns and a markdown table, both
    /// padded to a common edge deep in the line.
    fn aligned_body() -> Vec<String> {
        let mut v: Vec<String> = (0..12)
            .map(|i| {
                format!(
                    "{:>28}    {:>18}    {:>10}    trailing context column {i}",
                    "field_name_here", "value", "count"
                )
            })
            .collect();
        v.push("| column one | column two | column three |".to_string());
        v.push("| ---------- | ---------- | ----------- |".to_string());
        v
    }

    fn assert_intact(name: &str, text: &str) {
        let got = strip_columns(text);
        assert_eq!(
            got.join("\n"),
            text,
            "{name}: a single-column screen was truncated"
        );
    }

    #[test]
    fn test_opencode_screen_is_never_cut() {
        // opencode's own status bar is one 200-char line with three big
        // internal gaps — the strongest false-positive candidate there is.
        let bar = " ⬝⬝⬝⬝⬝⬝⬝⬝ esc interrupt                                                                                                                  145.6K (14%)  ctrl+p commands    ~/projects/herdr-telegram:main";
        let mut lines = aligned_body();
        lines.push(bar.to_string());
        lines.push("• OpenCode 1.18.31".to_string());
        assert_intact("opencode", &screen(&lines));
    }

    #[test]
    fn test_agy_screen_is_never_cut() {
        let mut lines = aligned_body();
        lines.extend([
            "Antigravity CLI 1.2.2".to_string(),
            "  ADC: firebase-adminsdk-fbsvc@test-proj-123".to_string(),
            "└ Tip: Run with --nocapture".to_string(),
            "▸ Subagents (1 running, 2 done)".to_string(),
        ]);
        assert_intact("agy", &screen(&lines));
    }

    #[test]
    fn test_claude_codex_and_gemini_screens_are_never_cut() {
        let mut lines = aligned_body();
        lines.extend([
            "● Bash(cargo test)".to_string(),
            "⎿ Done in 1.2s".to_string(),
            "? for shortcuts             Gemini 3.8 Flash · high".to_string(),
            "────────────────────────────────────────────────".to_string(),
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀".to_string(),
        ]);
        assert_intact("claude/codex/gemini", &screen(&lines));
    }

    /// The two-column case that motivated the cut must still work, so
    /// this battery cannot be satisfied by disabling it.
    #[test]
    fn test_two_column_pane_is_still_cut() {
        let edge = 120usize;
        let row = |l: &str, r: &str| {
            format!(
                "{l}{}{r}",
                " ".repeat(edge.saturating_sub(l.chars().count()))
            )
        };
        let text = [
            row("     Here is the answer to the question.", "▼ Context"),
            row("     It continues across a second line.", "Cache rate"),
            row(
                "     And a third, so the column agrees.",
                "src/main.rs  +4 -1",
            ),
        ]
        .join("\n");
        let got = strip_columns(&text).join("\n");
        for keep in ["answer to the question", "continues across", "a third"] {
            assert!(got.contains(keep), "conversation lost {keep:?}: {got:?}");
        }
        for junk in ["Context", "Cache rate", "main.rs"] {
            assert!(!got.contains(junk), "sidebar survived: {junk:?}");
        }
    }
}
