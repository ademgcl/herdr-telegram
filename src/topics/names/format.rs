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
/// e.g. `[shop] shop-backend · claude` (when labeled),
/// or `[tg] o2 · opencode` (when unlabeled, using assigned tag).
///
/// The agent suffix is the full agent name (`opencode`, `agy` — short
/// codes live only in tags like `o2`/`a14`). Overlong names truncate at
/// Telegram's 128-char cap. Shells take NO suffix:
/// the `sh<n>` tag already says shell (`[space-1] sh1`).
///
/// Minimal-dedup: a body echoing the space head word collapses (`ip
/// shell` in space `ip` → `[ip] shell`); an agent body that IS the
/// agent renders short — but ONLY when a strip already fired, so bare
/// bodies never newly collide (`opencode` stays `[tg] opencode · opencode`).
/// Near-identical labels can still converge (`foo` vs `foo · opencode` →
/// one title — identical labels always could; degenerate kind-word shell
/// bodies like `shell` vs `ip shell` likewise meet at `[ip] shell`).
/// Non-bracketed legacy labels carrying a short code (`o2 · o`) shed it
/// and re-sync; old bracketed titles are preserved as-is
/// (frozen when the herdr label itself is bracketed: desired equals
/// stored, so the watchdog never rewrites them).
///
/// Idempotent: if `label_or_tag` already begins with `[` (custom or previously
/// formatted title), it preserves it as-is without double-wrapping brackets.
/// Strips redundant trailing ` · {agent}`, ` · {short code}`, or ` · {space}`
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
    // shell (`[space-1] sh1`). Agents keep their full name.
    let clean_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        "shell" => String::new(),
        k => k.to_string(),
    };
    // Legacy short code (`o` for opencode): shed so old titles re-sync.
    let legacy_short: String = match kind_full.as_str() {
        "" | "?" | "shell" => String::new(),
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
    // Legacy short-code suffixes (`o2 · o` era): shed so they re-sync
    // to the full name. Current full suffix already stripped above.
    if !legacy_short.is_empty() && legacy_short != clean_agent {
        let legacy_suffix = format!(" · {legacy_short}");
        if let Some(stripped) = strip_suffix_ci(body, &legacy_suffix) {
            body = stripped;
            slimmed = true;
        }
    }
    // Shell bodies carrying the kind word (`sh1 · shell` era): shed.
    if clean_agent.is_empty()
        && let Some(stripped) = strip_suffix_ci(body, " · shell")
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

    // The slimmed body already says the agent: render it once instead
    // of stuttering (`[tg] opencode`). Gated on a fired strip so bare
    // bodies never newly collide (`[tg] opencode · opencode`).
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

/// Friendly default: `format_title(space, tag, kind)` — `[{space}] {tag} ·
/// {agent}` e.g. `[tg] o2 · opencode`. Tags (`o2`, `a14`, `sh1`) keep short
/// codes; only the title suffix uses the full agent name.
#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
