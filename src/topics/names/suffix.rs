//! Tolerant space-suffix strip for topic titles.
//!
//! Split from `chrome` (500-line file limit). The title renders the space
//! as a trailing qualifier (`2[tg]`), so the reverse pass must strip it
//! or a rename double-wraps and recovers the wrong label.
use super::chrome::norm_title;

/// Tolerant `core[space]` SUFFIX strip: the shape this build renders
/// (`2[tg]`), plus the transitional `2 . tg` and the tight `2.tg` a
/// user may paste, and the legacy `[space] core` for old titles re-saved.
///
/// `None` = not this space (a custom `core[other]` label keeps its own
/// trailing word as part of the name), so re-formatting is idempotent
/// without eating a user's label.
pub(crate) fn strip_space_suffix<'a>(body: &'a str, space: &str, short: &str) -> Option<&'a str> {
    let t = body.trim_end();
    // Bracketed qualifier: `core[space]`.
    if let Some(inner) = t.strip_suffix(']')
        && let Some((head, tail)) = inner.rsplit_once('[')
    {
        if !head.trim().is_empty() && is_this_space(tail, space, short) {
            return Some(head.trim_end());
        }
        return None;
    }
    // Transitional dotted form: `core . space` / `core.space`.
    let (head, tail) = t.rsplit_once('.')?;
    if head.trim().is_empty() {
        return None;
    }
    is_this_space(tail, space, short).then(|| head.trim_end())
}

pub(crate) fn is_this_space(tail: &str, space: &str, short: &str) -> bool {
    let n = norm_title(tail);
    n == norm_title(space.trim()) || n == norm_title(short)
}
