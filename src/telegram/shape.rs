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
/// Characters per paragraph before it is broken up. A phone shows ~40.
const PARA_MAX: usize = 200;

pub fn shape(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // Fenced code is opaque: never reflow it, never split it.
    let mut in_fence = false;
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
        if !in_fence && let Some(rows) = table_row(line) {
            out.extend(rows);
            continue;
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
        if p.chars().count() <= PARA_MAX {
            paras.push(p);
            continue;
        }
        for chunk in split_sentences(&p) {
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
    // Skip the HEADER row: its cells are repeated as the bullets.
    None.or(Some(cells.into_iter().map(|c| format!("• {c}")).collect()))
}

/// Break a long paragraph at sentence ends, so each chunk reads on its
/// own. Falls back to a word boundary if a "sentence" runs long.
fn split_sentences(p: &str) -> Vec<String> {
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
        } else if buf.chars().count() + 1 + s.chars().count() <= PARA_MAX {
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

    #[test]
    fn test_table_becomes_bullets() {
        let t = "| cmd | where |\n| --- | --- |\n| /new | topic |\n| /spawn | general |";
        let out = shape(t);
        assert!(out.contains("• /new"), "table not bulleted: {out}");
        assert!(out.contains("• topic"), "cell lost: {out}");
        assert!(!out.contains("---"), "divider survived: {out}");
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

/// The pref is what makes the formatter a choice rather than a
/// behaviour change: default on (unreadable is the bug), and flipping
/// it must pass the body through untouched.
#[cfg(test)]
mod pref_tests {
    use super::shape;

    #[tokio::test]
    async fn test_shape_flag_defaults_on_and_persists() {
        let (s, _dir) = crate::state::cancel::isolated_state();
        assert!(s.shape_telegram(), "readable replies are the default");
        s.set_shape_telegram(false).await;
        assert!(!s.shape_telegram(), "off is honoured in memory");
        assert!(!crate::state::State::load_shape_telegram(), "off persists");
        s.set_shape_telegram(true).await;
        assert!(s.shape_telegram());
        assert!(crate::state::State::load_shape_telegram());
    }

    /// The gate must be a no-op for the common case, so the common case
    /// costs nothing and cannot regress.
    #[test]
    fn test_shaping_a_short_reply_is_a_no_op() {
        let short = "Fixed. The card is the reply now — nothing else changed.";
        assert_eq!(shape(short), short);
    }
}
