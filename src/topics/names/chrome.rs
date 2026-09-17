//! Tolerant Format-B chrome parsing (shared): `[bracket]` split,
//! space-prefix strip + space-rename intent, code-suffix shedding,
//! stray trims. Split from `format` (300-line file limit).
use super::KIND_CODES;

/// Collapse whitespace + lowercase (unicode-aware): the single
/// comparison basis for space names, codes, and cover checks, so
/// `[my  space]`/`[MY SPACE]`/`[TG] API · O` all match intelligently.
pub(crate) fn norm_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Display space (20-char truncation, same as forward titles).
pub(crate) fn short_space_for(space: &str) -> String {
    let t = space.trim();
    if t.is_empty() || t == "?" {
        return "ws".to_string();
    }
    t.chars().take(20).collect()
}

const DOT_SEPS: [char; 3] = ['·', '•', '⋅'];
const ALT_SEPS: [char; 9] = ['|', ':', '/', '-', '‐', '‑', '‒', '–', '—'];

/// Split a leading `[bracket]` into (inner, remainder): `None` when the
/// text doesn't open with a closed `[...]` pair. Dumb on purpose —
/// callers decide what a match means (space rename vs custom title).
/// `]` is ASCII so the slices are always boundary-safe.
pub(crate) fn split_bracket(body: &str) -> Option<(&str, &str)> {
    let inner = body.trim_start().strip_prefix('[')?;
    let close = inner.find(']')?;
    Some((inner[..close].trim(), inner[close + 1..].trim_start()))
}

/// Space-rename intent from a Telegram rename: `Some((new_space,
/// pane_rest))` when the leading `[bracket]` names a DIFFERENT space
/// (tolerant: case-blind, whitespace-collapsed, full or displayed
/// 20-char truncation all count as "same"). `None` = no bracket, empty
/// brackets, or the same space — caller takes the pane/tab path.
/// The bracket names the space, the remainder names the pane: `[new]
/// label` renames the workspace to `new` (pane keeps `label`), never
/// the tab to `[new] label` (that duplication was the bug).
pub(crate) fn space_rename_parts(new_name: &str, space: &str) -> Option<(String, String)> {
    let sp = space.trim();
    if sp.is_empty() || sp == "?" {
        return None;
    }
    let (inner, rest) = split_bracket(new_name)?;
    if inner.is_empty() {
        return None;
    }
    let short = short_space_for(sp);
    if norm_title(inner) == norm_title(sp) || norm_title(inner) == norm_title(&short) {
        return None;
    }
    Some((inner.to_string(), rest.trim().to_string()))
}

/// Tolerant `[space]` prefix strip: case-blind, whitespace-collapsed,
/// full label or displayed 20-char truncation. `]` is ASCII so the
/// slice boundary is always safe. `None` = not this space (custom
/// `[bracket]` — caller keeps brackets as core, never strips).
pub(crate) fn strip_space_prefix<'a>(body: &'a str, space: &str, short: &str) -> Option<&'a str> {
    let (named, rest) = split_bracket(body)?;
    if named.is_empty() {
        return None;
    }
    let n = norm_title(named);
    if n == norm_title(space.trim()) || n == norm_title(short) {
        Some(rest)
    } else {
        None
    }
}

fn is_dot_sep(c: char) -> bool {
    DOT_SEPS.contains(&c)
}

/// Strip one trailing `· {token}` (dot variants, flexible spacing,
/// case-blind, whitespace-collapsed): `main·o`, `main  ·  O`,
/// `o2 · MY SPACE` all shed. Returns the core slice of `body`.
fn strip_dot_token<'a>(body: &'a str, token: &str) -> Option<&'a str> {
    if token.trim().is_empty() {
        return None;
    }
    let t = body.trim_end();
    let pos = t.char_indices().rev().find(|(_, c)| is_dot_sep(*c))?.0;
    let (before, after) = t.split_at(pos);
    let after = after.trim_start_matches(is_dot_sep).trim();
    if after.is_empty() || norm_title(after) != norm_title(token) {
        return None;
    }
    let core = before.trim_end();
    if core.is_empty() { None } else { Some(core) }
}

/// Split trailing dot-suffixed code: returns (core, token) when the
/// text after the last `·•⋅` is a plausible code (1–2 alnum, `agent`,
/// `$`, or a known full kind). Stale codes shed here so flips stay bare
/// instead of stacking (`main · x` under kind opencode → `main`).
fn split_dot_code(body: &str) -> Option<(&str, &str)> {
    let t = body.trim_end();
    if t.ends_with('$') {
        let cut = t.len() - 1;
        let before = t[..cut].trim_end().strip_suffix(|c: char| is_dot_sep(c) || c.is_whitespace())?;
        let before = before.trim_end();
        // Require a real dot separator just before (not a bare `$` name).
        let pre = t[..cut].trim_end();
        if !pre.char_indices().rev().any(|(_, c)| is_dot_sep(c)) {
            // Fall through to generic only if a dot exists earlier;
            // a lone trailing `$` without `·` is part of the name.
            return None;
        }
        let _ = before;
        // Re-split at the last dot for an exact core slice.
        let pos = t.char_indices().rev().find(|(_, c)| is_dot_sep(*c))?.0;
        let (b, a) = t.split_at(pos);
        let a = a.trim_start_matches(is_dot_sep).trim();
        if a == "$" && !b.trim().is_empty() {
            return Some((b.trim_end(), "$"));
        }
        return None;
    }
    let pos = t.char_indices().rev().find(|(_, c)| is_dot_sep(*c))?.0;
    let (before, after) = t.split_at(pos);
    let tok = after.trim_start_matches(is_dot_sep).trim();
    if tok.is_empty() {
        return None;
    }
    let n = norm_title(tok);
    let generic = tok.chars().count() <= 2 && tok.chars().all(|c| c.is_alphanumeric());
    if generic || n == "agent" || KIND_CODES.iter().any(|(f, _)| *f == n) {
        let core = before.trim_end();
        if !core.is_empty() {
            return Some((core, tok));
        }
    }
    None
}

/// Strip one trailing ` | {known}` / ` - {known}` / ` : {known}` /
/// ` / {known}` (spaces REQUIRED both sides, so `shop-backend` and
/// `a/b` never shed). Dots stay flexible above; alt seps only shed
/// known codes/space/full-kinds/single letters — `foo - v2` survives.
fn strip_alt_token<'a>(body: &'a str, token: &str) -> Option<&'a str> {
    if token.trim().is_empty() {
        return None;
    }
    let t = body.trim_end();
    let parts: Vec<&str> = t.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    if norm_title(parts[parts.len() - 1]) != norm_title(token) {
        return None;
    }
    let sep = parts[parts.len() - 2];
    if sep.chars().count() != 1 || !ALT_SEPS.contains(&sep.chars().next().unwrap()) {
        return None;
    }
    // Slice the original (preserve inner spacing): drop trailing
    // `<ws><sep><ws><token>`.
    let tok_start = t.trim_end().len() - parts[parts.len() - 1].len();
    let before_tok = t[..tok_start].trim_end();
    let sep_start = before_tok.len() - sep.len();
    let core = before_tok[..sep_start].trim_end();
    if core.is_empty() { None } else { Some(core) }
}

/// One shed step (dot-flexible knowns → generic dot code → alt knowns).
/// Returns the shortened core slice, or `None` when nothing sheds.
pub(crate) fn shed_once<'a>(
    body: &'a str,
    short_space: &str,
    kind_full: &str,
    short_agent: &str,
) -> Option<&'a str> {
    for cand in [short_agent, short_space, kind_full] {
        if !cand.trim().is_empty()
            && cand.trim() != "?"
            && let Some(s) = strip_dot_token(body, cand)
            && !s.is_empty()
        {
            return Some(s);
        }
    }
    if short_agent != "agent"
        && let Some(s) = strip_dot_token(body, "agent")
        && !s.is_empty()
    {
        return Some(s);
    }
    if let Some(s) = strip_dot_token(body, "$")
        && !s.is_empty()
    {
        return Some(s);
    }
    if let Some((core, _)) = split_dot_code(body) {
        return Some(core);
    }
    for cand in [short_agent, short_space, kind_full, "agent", "$"] {
        if !cand.trim().is_empty()
            && cand.trim() != "?"
            && let Some(s) = strip_alt_token(body, cand)
            && !s.is_empty()
        {
            return Some(s);
        }
    }
    for (f, s) in KIND_CODES {
        for k in [*f, *s] {
            if let Some(s) = strip_alt_token(body, k)
                && !s.is_empty()
            {
                return Some(s);
            }
        }
    }
    // Single ASCII letter after `| : / -` is near-certainly a code
    // (`| o`). ASCII-only (codes are ASCII) + byte-safe slicing: a
    // Unicode alnum (`é`) must never shed, and never panic.
    {
        let t = body.trim_end();
        let parts: Vec<&str> = t.split_whitespace().collect();
        if parts.len() >= 3 {
            let last = parts[parts.len() - 1];
            if last.len() == 1 && last.chars().all(|c| c.is_ascii_alphanumeric()) {
                let sep = parts[parts.len() - 2];
                if sep.chars().count() == 1 && ALT_SEPS.contains(&sep.chars().next().unwrap()) {
                    // Safe: `t` ends with `last` (trimmed), so the cut
                    // lands on a char boundary; `trim_end` absorbs any
                    // spacing runs (`main  |  q` sheds like `main | q`).
                    let before_tok = t[..t.len() - last.len()].trim_end();
                    if let Some(core) = before_tok.strip_suffix(sep) {
                        let core = core.trim_end();
                        if !core.is_empty() {
                            return Some(core);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Trailing stray separators (`ip ·`, `main |`, `a -`): never part of a
/// core. Bare `$` is part of the name (`main$` survives — only `· $`
/// sheds via `shed_once` above). Single pure trim (the outer shed loop
/// re-trims after each shed).
pub(crate) fn trim_stray(body: &str) -> &str {
    body.trim_end_matches(|c: char| {
        DOT_SEPS.contains(&c) || ALT_SEPS.contains(&c) || c.is_whitespace()
    })
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
