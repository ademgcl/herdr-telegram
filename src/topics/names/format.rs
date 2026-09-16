//! Format B titles `[space] body · agent` (shells bare: `[space] body`).
use super::code;

/// Case-insensitive trailing strip. Byte-safe: `.get()` rejects a cut
/// that isn't a char boundary (unreachable — a case-insensitive match
/// against an ASCII-folded suffix aligns boundaries — but free).
fn strip_suffix_ci<'a>(body: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = body.len().checked_sub(suffix.len())?;
    let tail = body.get(cut..)?;
    if tail.eq_ignore_ascii_case(suffix) {
        Some(body[..cut].trim())
    } else {
        None
    }
}

/// Case-insensitive leading-`word` strip: removes `word` only when it
/// is a standalone head word (followed by whitespace/`·`/end), so
/// compounds (`shop-backend` in space `shop`) survive. Only
/// whitespace/`·` count as boundaries — `-_:/` form real compounds
/// (branches, paths, snake_case) and must never strip. Char-wise and
/// byte-safe: each cut advances by the matched char's UTF-8 length.
fn strip_leading_word<'a>(body: &'a str, word: &str) -> Option<&'a str> {
    if word.is_empty() {
        return None;
    }
    let mut rest = body;
    for wc in word.chars() {
        let bc = rest.chars().next()?;
        if bc.to_lowercase().ne(wc.to_lowercase()) {
            return None;
        }
        rest = &rest[bc.len_utf8()..];
    }
    match rest.chars().next() {
        None => Some(rest),
        Some(sep) if sep.is_whitespace() || sep == '·' => {
            Some(rest.trim_start_matches(|c: char| c.is_whitespace() || c == '·'))
        }
        _ => None,
    }
}

/// Format B topic title: `[{space}] {label_or_tag} · {agent}`.
/// e.g. `[shop] shop-backend · c` (when labeled),
/// or `[tg] o2 · o` (when unlabeled, using assigned tag).
///
/// The agent suffix is the short [`code`] (`o` for opencode — the full
/// name buys nothing in a 128-char topic title). Shells take NO suffix:
/// the `sh<n>` tag already says shell (`[space-1] sh1`).
///
/// Minimal-dedup: a body echoing the space head word collapses (`ip
/// shell` in space `ip` → `[ip] shell`); an agent body that IS the
/// agent renders short — but ONLY when a strip already fired, so bare
/// bodies never newly collide (`o` stays `[tg] o · o`). Near-identical
/// labels can still converge (`foo` vs `foo · o` → one title —
/// identical labels always could; degenerate kind-word shell bodies
/// like `shell` vs `ip shell` likewise meet at `[ip] shell`).
/// Non-bracketed legacy labels carrying the full kind (`o2 · opencode`)
/// shed it and re-sync; old bracketed titles are preserved as-is
/// (frozen when the herdr label itself is bracketed: desired equals
/// stored, so the watchdog never rewrites them).
///
/// Idempotent: if `label_or_tag` already begins with `[` (custom or previously
/// formatted title), it preserves it as-is without double-wrapping brackets.
/// Strips redundant trailing ` · {agent}`, ` · {space}`, or ` · {full kind}`
/// suffixes (case-insensitive).
///
/// Telegram caps topic names at 128 UTF-8 characters; space is truncated to 20.
/// Prefix matching uses the DISPLAYED (possibly truncated) space.
pub fn format_title(space: &str, label_or_tag: &str, kind: &str) -> String {
    let trimmed = label_or_tag.trim();
    if trimmed.starts_with('[') {
        return trimmed.chars().take(128).collect();
    }

    let raw_space = space.trim();
    let short_space: String = if raw_space.is_empty() || raw_space == "?" {
        "ws".to_string()
    } else {
        raw_space.chars().take(20).collect()
    };

    let kind_full = kind.trim().to_lowercase();
    // Shells carry no agent suffix at all: the `sh<n>` tag already says
    // shell (`[space-1] sh1`). Agents keep their short code.
    let clean_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        "shell" => String::new(),
        k => code(k),
    };

    // Trailing stray separators first (`ip ·` → `ip`, never `· ·`).
    // A degenerate all-separator body keeps its trimmed text (stable,
    // never an empty title).
    let mut body = trimmed.trim_end_matches(|c: char| c == '·' || c.is_whitespace());
    if body.is_empty() {
        return trimmed.chars().take(128).collect();
    }
    // `slimmed` gates the suffix-drop: without a fired strip, distinct
    // panes must keep distinct titles (see doc).
    let mut slimmed = false;
    if !clean_agent.is_empty() {
        let agent_suffix = format!(" · {clean_agent}");
        if let Some(stripped) = strip_suffix_ci(body, &agent_suffix) {
            body = stripped;
            slimmed = true;
        }
    }
    let space_suffix = format!(" · {short_space}");
    if let Some(stripped) = strip_suffix_ci(body, &space_suffix) {
        body = stripped;
        slimmed = true;
    }
    if !kind_full.is_empty() && kind_full != "?" {
        let full_suffix = format!(" · {kind_full}");
        // A suffix-less kind (shell) always differs here; agents skip
        // re-stripping their own short suffix.
        let short_suffix = format!(" · {clean_agent}");
        if full_suffix != short_suffix
            && let Some(stripped) = strip_suffix_ci(body, &full_suffix)
        {
            body = stripped;
            slimmed = true;
        }
        // Retired one-char suffixes (`$` era): shed so they re-sync.
        if clean_agent.is_empty()
            && let Some(stripped) = strip_suffix_ci(body, " · $")
        {
            body = stripped;
        }
    }

    // Head-word echoes collapse to a fixed point (`ip ip shell` → `shell`).
    // Empty remainder keeps the previous core (stable, unambiguous).
    let mut core = body;
    loop {
        match strip_leading_word(core, &short_space) {
            Some(rest) if !rest.trim().is_empty() => {
                core = rest.trim();
                slimmed = true;
            }
            _ => break,
        }
    }

    // Suffix-less kinds (shell) are done: the tag says it all
    // (`[space-1] sh1`). Degenerate kind-word bodies (`shell` vs
    // `ip shell`) converge — near-identical labels always could.
    if clean_agent.is_empty() {
        let minimal: String = format!("[{short_space}] {core}");
        return minimal.chars().take(128).collect();
    }

    // The slimmed body already says the agent: render it short instead
    // of stuttering (`[tg] o`). Gated on a fired strip so bare bodies
    // never newly collide (`[tg] o · o`).
    if slimmed {
        if core.eq_ignore_ascii_case(&clean_agent) {
            let minimal: String = format!("[{short_space}] {core}");
            return minimal.chars().take(128).collect();
        }
        if !kind_full.is_empty()
            && kind_full != "?"
            && core.eq_ignore_ascii_case(kind_full.as_str())
        {
            let minimal: String = format!("[{short_space}] {clean_agent}");
            return minimal.chars().take(128).collect();
        }
    }

    let formatted = format!("[{short_space}] {core} · {clean_agent}");
    formatted.chars().take(128).collect()
}

/// Friendly default title in Format B: `[{space}] {tag} · {code}` e.g. `[tg] o2 · o`.
/// Written into the herdr pane label when unlabeled, so the default name is a
/// real herdr-tracked name, not just a telegram string.
pub fn title(tag: &str, space: &str, kind: &str) -> String {
    format_title(space, tag, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_format() {
        assert_eq!(title("o2", "tg", "opencode"), "[tg] o2 · o");
        assert_eq!(title("c1", "ajnow", "claude"), "[ajnow] c1 · c");
        assert_eq!(
            title("o1", "a-very-long-workspace-label-here", "opencode"),
            "[a-very-long-workspac] o1 · o"
        );
        assert_eq!(
            format_title("shop", "shop-backend", "claude"),
            "[shop] shop-backend · c"
        );
        // Legacy bracketed titles freeze verbatim (stable: no rewrite loop).
        assert_eq!(
            format_title("shop", "[shop] shop-backend · claude", "claude"),
            "[shop] shop-backend · claude"
        );
        assert_eq!(format_title("tg", "o2 · tg", "opencode"), "[tg] o2 · o");
        // Shells take no suffix: the sh tag says it all.
        assert_eq!(format_title("space-1", "sh1", "shell"), "[space-1] sh1");
        assert_eq!(format_title("infra", "s1", "shell"), "[infra] s1");
        // Unknown kinds fall back to their 2-letter code; blank stays literal.
        assert_eq!(format_title("tg", "x1", "my-agent"), "[tg] x1 · my");
        assert_eq!(format_title("tg", "x1", "?"), "[tg] x1 · agent");
        assert_eq!(
            format_title("herdr-telegram", "main · herdr-telegram imac", "opencode"),
            "[herdr-telegram] main · herdr-telegram imac · o"
        );
        // Minimal-dedup: the reported stutter collapses...
        assert_eq!(format_title("ip", "ip shell", "shell"), "[ip] shell");
        assert_eq!(format_title("ip", "IP SHELL", "shell"), "[ip] SHELL");
        assert_eq!(format_title("ip", "ip ip shell", "shell"), "[ip] shell");
        assert_eq!(format_title("ip", "ip ·", "shell"), "[ip] ip");
        assert_eq!(format_title("tg", "tg opencode", "opencode"), "[tg] o");
        // ...legacy full-kind labels shed the kind and re-sync...
        assert_eq!(
            format_title("tg", "o2 · opencode", "opencode"),
            "[tg] o2 · o"
        );
        assert_eq!(format_title("ip", "sh1 · shell", "shell"), "[ip] sh1");
        assert_eq!(format_title("ip", "foo · SHELL", "shell"), "[ip] foo");
        assert_eq!(format_title("ip", "sh1 · $", "shell"), "[ip] sh1");
        // ...kind match is case-blind (herdr kinds are lowercase anyway)...
        assert_eq!(format_title("ip", "sh1", "SHELL"), "[ip] sh1");
        // ...but agents keep the gate: bare bodies never newly collide...
        assert_eq!(format_title("tg", "o", "opencode"), "[tg] o · o");
        assert_eq!(
            format_title("tg", "opencode", "opencode"),
            "[tg] opencode · o"
        );
        // ...degenerate kind-word shell bodies converge (documented)...
        assert_eq!(format_title("ip", "shell", "shell"), "[ip] shell");
        // ...exact-space bodies stay (stable, unambiguous)...
        assert_eq!(format_title("ip", "ip", "shell"), "[ip] ip");
        // ...and compounds are untouched.
        assert_eq!(
            format_title("shop", "shop-backend", "shell"),
            "[shop] shop-backend"
        );
        assert_eq!(format_title("ip", "ip: shell", "shell"), "[ip] ip: shell");
    }

    #[test]
    fn test_title_fixed_point() {
        // Re-formatting an output never changes it (watchdog converges).
        for (space, label, kind) in [
            ("ip", "ip shell", "shell"),
            ("ip", "sh1 · shell", "shell"),
            ("ip", "foo · SHELL", "shell"),
            ("ip", "shell", "shell"),
            ("tg", "o", "opencode"),
            ("tg", "o2 · opencode", "opencode"),
            ("shop", "shop-backend", "claude"),
            ("ip", "ip", "shell"),
        ] {
            let once = format_title(space, label, kind);
            assert_eq!(format_title(space, &once, kind), once, "not stable: {once}");
        }
    }

    #[test]
    fn test_strip_leading_word() {
        assert_eq!(strip_leading_word("ip shell", "ip"), Some("shell"));
        assert_eq!(strip_leading_word("ip", "ip"), Some(""));
        assert_eq!(strip_leading_word("IP · x", "ip"), Some("x"));
        // Compounds, mismatches, and empty words never strip.
        assert_eq!(strip_leading_word("shop-backend", "shop"), None);
        assert_eq!(strip_leading_word("ipx", "ip"), None);
        assert_eq!(strip_leading_word("x shell", "ip"), None);
        assert_eq!(strip_leading_word("ip shell", ""), None);
        // Non-ASCII is char-safe (no panic, correct split).
        assert_eq!(strip_leading_word("büro fax", "büro"), Some("fax"));
        assert_eq!(strip_leading_word("bürofax", "büro"), None);
        // Multi-char lowercase folds only miss, never false-strip.
        assert_eq!(strip_leading_word("İstanbul x", "istanbul"), None);
    }
}
