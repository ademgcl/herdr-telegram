//! Format B titles `[space] body · code` (e.g. `[tg] o2 · o`).
use super::chrome::{shed_once, short_space_for, strip_space_prefix, trim_stray};
use super::code;

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
/// 1:1 always: every herdr label renders with its space + live kind
/// code, so a Telegram rename to `Custom` converges to
/// `[space] Custom · o/a…` — space and agent code are never lost.
/// Custom `[bracket]` text is kept INSIDE the wrap
/// (`[urgent] fix` → `[shop] [urgent] fix · o`), never frozen.
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
/// Pasted chrome sheds tolerantly and re-syncs to short: any `·•⋅`
/// code (any 1–2 alnum, stale or current), `· agent`, `· $`,
/// `· {space}`, legacy full kinds, plus `| : / -` variants with spaces
/// (`main | o`, `main - o`); case-blind, whitespace-collapsed.
///
/// Idempotent: `format_title(space, &format_title(space, l, k), k)`
/// is stable (space prefix + chrome shed, then re-wrapped).
///
/// Telegram caps topic names at 128 UTF-8 characters; space is truncated to 20.
/// Prefix matching uses the DISPLAYED (possibly truncated) space.
pub fn format_title(space: &str, label_or_tag: &str, kind: &str) -> String {
    let trimmed = label_or_tag.trim();
    let short_space = short_space_for(space);
    let kind_full = kind.trim().to_lowercase();
    // Short kind code on every title (`o`, `a`, `sh` — shell included:
    // the PC shows live kind, Telegram only has this suffix). Unknown
    // kinds keep the explicit `agent` word.
    let short_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        k => code(k),
    };

    // A pasted rendered title unwraps first (`[space] core` → `core`)
    // so re-formatting never double-wraps; custom `[bracket]` names
    // (not this space) stay whole and gain the wrap below.
    let mut body = match strip_space_prefix(trimmed, space.trim(), &short_space) {
        Some(rest) if !rest.trim().is_empty() => rest.trim(),
        _ => trimmed,
    };
    // Unclosed `[tg main` drops the stray bracket into the normal path.
    if body.starts_with('[') && !body.contains(']') {
        body = body.trim_start_matches(['[', ' ']);
        if body.trim().is_empty() {
            return trimmed.chars().take(128).collect();
        }
        body = body.trim();
    }
    // Trailing strays + pasted chrome shed to a fixed point (stacked
    // pastes shed fully; each step strictly shrinks, always terminates).
    // A degenerate all-separator body keeps its trimmed text (stable,
    // never an empty title).
    body = trim_stray(body);
    if body.is_empty() {
        return trimmed.chars().take(128).collect();
    }
    loop {
        let len = body.len();
        body = trim_stray(body);
        if let Some(s) = shed_once(body, &short_space, &kind_full, &short_agent)
            && !s.trim().is_empty()
        {
            body = s.trim();
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
    if core.trim().is_empty() {
        return trimmed.chars().take(128).collect();
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
