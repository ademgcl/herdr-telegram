//! Format B titles `[space] body · code` (e.g. `[tg] o2 · o`).
use super::code;

/// Case-insensitive trailing strip. Byte-safe: `.get()` rejects a cut
/// that isn't a char boundary (unreachable — a case-insensitive match
/// against an ASCII-folded suffix aligns boundaries — but free).
/// Shared with the reverse pass ([`super::core`]).
pub(crate) fn strip_suffix_ci<'a>(body: &'a str, suffix: &str) -> Option<&'a str> {
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
/// Shared with the reverse pass ([`super::core`]).
pub(crate) fn strip_leading_word<'a>(body: &'a str, word: &str) -> Option<&'a str> {
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

/// Format B topic title: `[{space}] {label_or_tag} · {code}`.
/// e.g. `[shop] shop-backend · c` (when labeled),
/// or `[tg] o2 · o` (when unlabeled, using assigned tag).
///
/// The kind suffix is the SHORT code (`o`, `a`, `sh` — the same map as
/// tags): Telegram can't show live kind like the PC tab bar, so the
/// title carries it and tracks flips (opencode → shell → agy each rename
/// once and converge). Unknown kinds keep the explicit `agent` word.
/// Overlong names truncate at Telegram's 128-char cap.
///
/// Minimal-dedup: a body echoing the space head word collapses (`ip
/// shell` in space `ip` → `[ip] shell · sh`); a body that IS the short
/// code renders once (`o` stays `[tg] o`, never `[tg] o · o`).
/// Near-identical labels can still converge (`foo` vs `foo · o` →
/// one title — identical labels always could).
/// Legacy full-name suffixes (`· opencode` era) shed and re-sync to
/// short; old bracketed titles are preserved as-is
/// (frozen when the herdr label itself is bracketed: desired equals
/// stored, so the watchdog never rewrites them).
///
/// Idempotent: if `label_or_tag` already begins with `[` (custom or previously
/// formatted title), it preserves it as-is without double-wrapping brackets.
/// Strips redundant trailing ` · {code}`, ` · {full kind}` (legacy era),
/// ` · agent`, or ` · {space}` suffixes (case-insensitive).
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
    // Short kind code on every title (`o`, `a`, `sh` — shell included:
    // the PC shows live kind, Telegram only has this suffix). Unknown
    // kinds keep the explicit `agent` word.
    let short_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        k => code(k),
    };

    // Trailing stray separators first (`ip ·` → `ip`, never `· ·`).
    // A degenerate all-separator body keeps its trimmed text (stable,
    // never an empty title).
    let mut body = trimmed.trim_end_matches(|c: char| c == '·' || c.is_whitespace());
    if body.is_empty() {
        return trimmed.chars().take(128).collect();
    }
    // Shed order: current short, space, legacy full kind (`· opencode`
    // era → short), legacy `· agent`, retired `$`. Each keeps a
    // non-empty core; the loop strictly shrinks, so stacked pastes shed
    // fully and always terminate.
    loop {
        let len = body.len();
        let short_suffix = format!(" · {short_agent}");
        if let Some(stripped) = strip_suffix_ci(body, &short_suffix)
            && !stripped.is_empty()
        {
            body = stripped;
        }
        let space_suffix = format!(" · {short_space}");
        if let Some(stripped) = strip_suffix_ci(body, &space_suffix)
            && !stripped.is_empty()
        {
            body = stripped;
        }
        if !kind_full.is_empty()
            && kind_full != "?"
            && kind_full != short_agent
            && let Some(stripped) =
                strip_suffix_ci(body, &format!(" · {kind_full}"))
            && !stripped.is_empty()
        {
            body = stripped;
        }
        if short_agent != "agent"
            && let Some(stripped) = strip_suffix_ci(body, " · agent")
            && !stripped.is_empty()
        {
            body = stripped;
        }
        if let Some(stripped) = strip_suffix_ci(body, " · $")
            && !stripped.is_empty()
        {
            body = stripped;
        }
        if body.len() == len {
            break;
        }
    }

    // Head-word echoes collapse to a fixed point (`ip ip shell` → `shell`).
    // Empty remainder keeps the previous core (stable, unambiguous).
    let mut core = body;
    loop {
        match strip_leading_word(core, &short_space) {
            Some(rest) if !rest.trim().is_empty() => {
                core = rest.trim();
            }
            _ => break,
        }
    }
    // A core that IS the short code renders once (`[tg] o`, `[ip] sh`):
    // the suffix would only stutter. Cross-kind same-label collisions
    // need identical labels with colliding shorts — documented
    // convergence, same class as identical labels always could.
    if core.eq_ignore_ascii_case(&short_agent) {
        let minimal: String = format!("[{short_space}] {core}");
        return minimal.chars().take(128).collect();
    }

    let formatted = format!("[{short_space}] {core} · {short_agent}");
    formatted.chars().take(128).collect()
}

/// Friendly default: `format_title(space, tag, kind)` — `[{space}] {tag} ·
/// {code}` e.g. `[tg] o2 · o`. Tags (`o2`, `a14`, `sh1`) keep short
/// codes; the title suffix uses the same short map.
#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
