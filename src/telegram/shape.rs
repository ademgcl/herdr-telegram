//! Shape a reply for a phone, without a model.
//!
//! Telegram already wraps text, so re-wrapping helps nobody. What hurts
//! on a small screen is structural: 200-character paragraphs, tables
//! that don't render, and blank-line pile-ups. Those are mechanical, so
//! fixing them costs nothing and cannot hallucinate.
//!
//! Deliberately conservative — it only rearranges, never summarises or
//! deletes. A formatter that drops a sentence is worse than a long
//! message, so anything it is not sure about is left exactly as written.
//!
//! Out of scope, needing a model: cutting waffle, or rewriting a
//! paragraph into one line. That is the standalone CLI's job.
/// Characters per paragraph before it is broken up (a phone shows ~40).
/// The default for `shape`; the CLI exposes it as `--max-para`, so the
/// number lives here once rather than being hard-coded in two places.
pub const PARA_MAX: usize = 200;

#[allow(dead_code)] // used by the bot; the CLI only needs `shape_with`
pub fn shape(text: &str) -> String {
    shape_with(text, PARA_MAX)
}

/// [`shape`] with an explicit paragraph chunk size, so a caller (the
/// `tgshape` CLI) can tune it without forking the rules.
pub fn shape_with(text: &str, para_max: usize) -> String {
    let _ = para_max;
    let mut out: Vec<String> = Vec::new();
    // Fenced code is opaque: never reflow it, never split it.
    let mut in_fence = false;
    let mut seen_table_header = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        // Order matters: blank and table rows are structural, so they are
        // classified BEFORE the prose path — a short table row is still a
        // table row.
        if line.trim().is_empty() && !in_fence {
            // Collapse runs of blank lines: three is noise, one is a break.
            if !out.last().is_some_and(|l: &String| l.trim().is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        // Table before atomic: a row starts with `|`, which is also a
        // structural marker, so atomic would swallow it.
        if !in_fence && let Some(cells) = table_row(line) {
            // The first row names the columns. Bulleting it just
            // duplicates them above the data, so keep it as a label.
            if !seen_table_header {
                seen_table_header = true;
                out.push(cells.join(" · "));
            } else if !cells.is_empty() {
                // One bullet per ROW, cells joined — a bullet per cell
                // scatters one answer across three lines. The
                // `|---|---|` divider yields no cells and adds nothing.
                out.push(format!("• {}", cells.join(" · ")));
            }
            continue;
        }
        // A new block resets the table state, so a second table on its
        // own gets its own header row.
        if !line.trim_start().starts_with('|') {
            seen_table_header = false;
        }
        if in_fence || is_atomic(line) {
            out.push(line.to_string());
            continue;
        }
        // A plain prose line: accumulate into a paragraph, then emit it
        // in short chunks.
        match out.last_mut() {
            Some(last) if !last.trim().is_empty() && !is_atomic(last) => {
                last.push(' ');
                last.push_str(line.trim());
            }
            _ => out.push(line.trim().to_string()),
        }
    }
    let mut paras: Vec<String> = Vec::new();
    for p in out {
        if is_atomic(&p) || p.trim().is_empty() {
            paras.push(p);
            continue;
        }
        if p.chars().count() <= para_max {
            paras.push(p);
            continue;
        }
        for chunk in split_sentences(&p, para_max) {
            paras.push(chunk);
        }
    }
    while paras.last().is_some_and(|l: &String| l.trim().is_empty()) {
        paras.pop();
    }
    paras.join("\n").trim().to_string()
}

/// Lines that must survive byte-for-byte: lists, headings, quotes,
/// separators and rules. Purely STRUCTURAL — length is not a signal
/// here, or a short table row would be mistaken for prose. Long prose is
/// split later, at chunking time.
fn is_atomic(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    // Ordered-list markers too: without this, "1. one" and "2. two" are
    // prose and get MERGED into one line. That is corruption, not
    // formatting, so it must not be missed.
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &t[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return true;
        }
    }
    // Bullets, numbers, headings, quotes, rules, and the chrome the
    // agent's own output uses.
    t.starts_with([
        '-', '*', '+', '>', '#', '|', '=', '~', '`', '┃', '│', '⬝', '$', '…',
    ]) || t
        .chars()
        .all(|c| c.is_whitespace() || matches!(c, '-' | '=' | '*' | '_' | '`' | '·' | '—'))
}

/// A markdown table row → bullet cells, so it reads on a phone.
/// Returns None for anything that is not a table row.
fn table_row(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.starts_with('|') || t.matches('|').count() < 2 {
        return None;
    }
    // The `|---|---|` divider becomes nothing.
    if t.chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c.is_whitespace())
    {
        return Some(Vec::new());
    }
    let cells: Vec<String> = t
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect();
    if cells.len() < 2 {
        return None;
    }
    Some(cells)
}

/// Break a long paragraph at sentence ends, so each chunk reads on its
/// own. Falls back to a word boundary if a "sentence" runs long.
fn split_sentences(p: &str, para_max: usize) -> Vec<String> {
    let mut sents: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in p.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '?' | '!') {
            sents.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        sents.push(cur.trim().to_string());
    }
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for s in sents {
        if buf.is_empty() {
            buf = s;
        } else if buf.chars().count() + 1 + s.chars().count() <= para_max {
            buf.push(' ');
            buf.push_str(&s);
        } else {
            out.push(std::mem::take(&mut buf));
            buf = s;
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::shape;

    #[test]
    fn test_short_reply_is_untouched() {
        let s = "Fixed it — the card is the reply now.";
        assert_eq!(shape(s), s, "a short reply must not be touched");
    }

    #[test]
    fn test_long_paragraph_is_broken_into_breaths() {
        let long = "The parser is the real cause and it only shows up on a wide terminal where the sidebar shares every line with the conversation. The fix is to cut at the column the screen agrees on, which is stable even when the gap before it is not.";
        let out = shape(long);
        assert!(out.lines().count() >= 2, "must split: {out}");
        for l in out.lines() {
            assert!(l.chars().count() <= 210, "chunk too long: {l:?}");
        }
        // Nothing lost: every word of the input survives.
        for w in long.split_whitespace() {
            assert!(out.contains(w.trim_end_matches(['.', ','])), "lost {w:?}");
        }
    }

    /// One bullet per ROW, not per cell: a bullet per cell scatters one
    /// answer across three lines, which is worse on a phone than the
    /// table was.
    #[test]
    fn test_table_becomes_one_bullet_per_row() {
        let t = "| cmd | where |\n| --- | --- |\n| /new | topic |\n| /shape | any |";
        let out = shape(t);
        assert_eq!(out, "cmd · where\n• /new · topic\n• /shape · any", "{out}");
        assert!(!out.contains("---"), "divider survived: {out}");
    }

    /// Two tables in one message must each get their own header — the
    /// second table's header row is data to the first.
    #[test]
    fn test_second_table_gets_its_own_header() {
        let t = "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n| c | d |\n| --- | --- |\n| 3 | 4 |";
        let out = shape(t);
        assert!(
            out.contains("a · b") && out.contains("c · d"),
            "headers: {out}"
        );
    }

    /// Ordered-list items must never be merged into one another. Getting
    /// this wrong is CORRUPTION, not formatting: "1. a" and "2. b" became
    /// "1. a 2. b".
    #[test]
    fn test_ordered_list_items_are_never_merged() {
        for sep in ["1. one\n2. two\n", "1) one\n2) two\n"] {
            let out = shape(sep);
            let first = sep.lines().next().unwrap();
            assert!(out.contains(first), "first item lost: {out}");
            assert!(out.lines().any(|l| l.starts_with("2")), "merged: {out}");
        }
    }

    /// The one guarantee that matters: a formatter that drops content is
    /// worse than a long message.
    #[test]
    fn test_no_content_is_ever_lost() {
        let inputs = [
            "The parser is the real cause and it only shows up on a wide terminal where the sidebar shares every line with the conversation. The fix is to cut at the column the screen agrees on.",
            "| cmd | where |\n| --- | --- |\n| /new | topic |",
            "```\nlet x = 1;\n```\nDone.",
            "- a\n1. b\n> c\n**d**",
        ];
        for i in inputs {
            let out = shape(i);
            for w in i
                .split_whitespace()
                .filter(|w| w.chars().any(char::is_alphanumeric))
            {
                let bare = w.trim_matches(|c: char| !c.is_alphanumeric());
                assert!(
                    !bare.is_empty() && out.contains(bare),
                    "lost {bare:?} from {i:?}"
                );
            }
        }
    }

    #[test]
    fn test_code_fence_is_byte_for_byte() {
        let code = "Here:\n```rust\nlet x = 1;\nlet very_long_line = some_call(a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q, r, s, t, u, v);\n```\nDone.";
        let out = shape(code);
        for l in code.lines() {
            assert!(out.contains(l), "code line altered: {l:?}\n{out}");
        }
    }

    #[test]
    fn test_lists_and_headings_survive() {
        let s = "Three things:\n- first item that is quite long and goes on and on and on for a while\n- second\n\n1. numbered one\n2. two\n\n**Bold** heading\n> quote";
        let out = shape(s);
        for l in [
            "- first",
            "- second",
            "1. numbered",
            "2. two",
            "**Bold**",
            "> quote",
        ] {
            assert!(out.contains(l), "lost {l:?}: {out}");
        }
    }

    #[test]
    fn test_blank_line_runs_collapse() {
        let out = shape("a\n\n\n\n\nb");
        assert_eq!(out, "a\n\nb", "blank pile-up not collapsed: {out:?}");
    }

    #[test]
    fn test_never_returns_empty_for_non_empty_input() {
        for s in ["word", "a b c", "- x\n- y", "```\ncode\n```"] {
            assert!(!shape(s).is_empty(), "lost everything: {s:?}");
        }
    }
}
