//! Reverse of Format-B titles: recover the herdr tab/pane core from a
//! Telegram topic name that may carry rendering chrome. Split from
//! `format` (300-line file limit).
use super::code;
use super::format::{strip_leading_word, strip_suffix_ci};

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;

/// One leading `[space]` prefix, case-blind, ONLY when the brackets name
/// this space (full label or the displayed 20-char truncation) — custom
/// `[bracket]` names never strip. Returns the remainder or `None`.
/// `]` is ASCII so the slice boundary is always safe.
fn strip_space_prefix<'a>(body: &'a str, space: &str, short: &str) -> Option<&'a str> {
    let t = body.trim_start();
    let inner = t.strip_prefix('[')?;
    let close = inner.find(']')?;
    let named = inner[..close].trim();
    if named.is_empty() {
        return None;
    }
    if named.eq_ignore_ascii_case(space.trim()) || named.eq_ignore_ascii_case(short) {
        Some(inner[close + 1..].trim_start())
    } else {
        None
    }
}

/// Recover the herdr tab/pane core from a Telegram topic name that may
/// carry Format-B chrome — users edit the rendered title, so renames
/// arrive decorated (`[space] main · o` must map back to `main`;
/// herdr already shows the space). Suffix sheds + head-word collapse
/// mirror the forward pass, so `topic_core(format_title(s, l, k)) == l`
/// for plain labels. Legacy full-name suffixes (`· opencode` era) shed
/// too, so pre-migration titles map back the same way. A leading
/// `[bracket]` that does NOT name this space is a custom name: returned
/// verbatim (the forward pass freezes such titles too, so both sides
/// agree). Never empty: falls back to the trimmed input.
pub fn topic_core(raw: &str, space: &str, kind: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let sp = space.trim();
    let short: String = if sp.is_empty() || sp == "?" {
        "ws".to_string()
    } else {
        sp.chars().take(20).collect()
    };
    let mut body = match strip_space_prefix(trimmed, sp, &short) {
        Some(rest) => rest,
        // Custom `[bracket]` name, not this space: verbatim, exactly
        // like the forward freeze — suffixes are part of the name.
        None if trimmed.starts_with('[') => return trimmed.to_string(),
        None => trimmed,
    };
    if body.is_empty() {
        return trimmed.to_string();
    }
    let kind_full = kind.trim().to_lowercase();
    // Same short map as the forward suffix (`o`, `a`, `sh`).
    let short_agent: String = match kind_full.as_str() {
        "" | "?" => "agent".to_string(),
        k => code(k),
    };
    // Same shed order as the forward pass, to a fixed point (stacked
    // pastes shed fully); each shed keeps a non-empty core, so the loop
    // strictly shrinks and always terminates.
    loop {
        let len = body.len();
        let short_suffix = format!(" · {short_agent}");
        if let Some(s) = strip_suffix_ci(body, &short_suffix)
            && !s.is_empty()
        {
            body = s;
        }
        let space_suffix = format!(" · {short}");
        if let Some(s) = strip_suffix_ci(body, &space_suffix)
            && !s.is_empty()
        {
            body = s;
        }
        // Legacy full-kind suffix (`· opencode` era → short now).
        if !kind_full.is_empty()
            && kind_full != "?"
            && kind_full != short_agent
            && let Some(s) = strip_suffix_ci(body, &format!(" · {kind_full}"))
            && !s.is_empty()
        {
            body = s;
        }
        // Legacy `· agent` (unknown-era titles re-labeled to a known kind).
        if short_agent != "agent"
            && let Some(s) = strip_suffix_ci(body, " · agent")
            && !s.is_empty()
        {
            body = s;
        }
        if let Some(s) = strip_suffix_ci(body, " · $")
            && !s.is_empty()
        {
            body = s;
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
