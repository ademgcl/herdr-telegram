//! Reverse of Format-B titles: recover the herdr tab/pane core from a
//! Telegram topic name that may carry rendering chrome. Split from
//! `format` (300-line file limit).
use super::chrome::{shed_once, short_space_for, strip_space_prefix, trim_stray};
use super::format::strip_leading_word;
use super::code;

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;

/// Recover the herdr tab/pane core from a Telegram topic name that may
/// carry Format-B chrome — users edit the rendered title, so renames
/// arrive decorated (`[space] main · o` must map back to `main`;
/// herdr already shows the space). Suffix sheds + head-word collapse
/// mirror the forward pass, so `topic_core(format_title(s, l, k)) == l`
/// for plain labels. 1:1 rule: space prefix (tolerant: case-blind,
/// whitespace-collapsed, truncated) + ANY pasted code chrome shed
/// (current, stale, `· agent`, `· $`, `· {space}`, legacy full kinds,
/// `| : / -` spaced variants, dot variants without spaces); custom
/// `[bracket]` text survives as core (it gains the wrap forward).
/// Unclosed `[tg main` drops the stray bracket instead of freezing.
/// Never empty: falls back to the trimmed input.
pub fn topic_core(raw: &str, space: &str, kind: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let sp = space.trim();
    let short = short_space_for(sp);
    let mut body = match strip_space_prefix(trimmed, sp, &short) {
        Some(rest) if !rest.trim().is_empty() => rest.trim(),
        // Custom `[bracket]` (not this space): keep whole as core so a
        // pasted ` · code` still sheds below; unclosed `[tg main`
        // drops the stray `[` into the normal path below.
        _ if trimmed.starts_with('[') && !trimmed.contains(']') => {
            let b = trimmed.trim_start_matches(['[', ' ']).trim();
            if b.is_empty() {
                return trimmed.to_string();
            }
            b
        }
        _ => trimmed,
    };
    if body.trim().is_empty() {
        return trimmed.to_string();
    }
    let kind_full = kind.trim().to_lowercase();
    // Same short map as the forward suffix (`o`, `a`, `sh`).
    let short_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        k => code(k),
    };
    // Tolerant shed to a fixed point (stacked pastes, stale codes shed
    // fully); each shed keeps a non-empty core, so the loop strictly
    // shrinks and always terminates. `let`-chains need Rust 2024.
    body = trim_stray(body);
    loop {
        let len = body.len();
        body = trim_stray(body);
        if let Some(s) = shed_once(body, &short, &kind_full, &short_agent)
            && !s.trim().is_empty()
        {
            body = s.trim();
        }
        if body.len() == len {
            break;
        }
    }
    // Head-word echoes collapse to a fixed point, like the forward pass
    // (`ip shell` in space `ip` → `shell`); compounds survive.
    loop {
        match strip_leading_word(body, &short) {
            Some(rest) if !rest.trim().is_empty() => {
                body = rest.trim();
            }
            _ => break,
        }
    }
    if body.trim().is_empty() {
        return trimmed.to_string();
    }
    body.trim().to_string()
}
