//! Pure title-decision rules (no I/O): tab→core picking, verbatim
//! preservation, `[new-space]` rename core + duplicate guard, and the
//! reset Step-4 choice. Split from `titles` (300-line file limit).
use crate::{topics::names::norm_title, types::WorkspaceInfo};
use std::collections::HashMap;

/// Count panes per tab_id (split-tab guard): siblings sharing one tab
/// disambiguate titles with the tag. One census per pass — shared by
/// the watchdog tick and reset loops so the counts can never drift.
pub fn tab_census(
    facts: &HashMap<String, crate::herdr::labels::PaneFacts>,
) -> HashMap<&str, usize> {
    let mut tab_count: HashMap<&str, usize> = HashMap::new();
    for f in facts.values() {
        if !f.tab_id.is_empty() {
            *tab_count.entry(f.tab_id.as_str()).or_default() += 1;
        }
    }
    tab_count
}

/// Resolve a pane's tab name + split flag from facts: the single source
/// for watchdog, adopt, and reset paths.
pub fn tab_of<'a>(
    facts: &'a HashMap<String, crate::herdr::labels::PaneFacts>,
    tabs: &'a HashMap<String, String>,
    census: &HashMap<&str, usize>,
    pane: &str,
) -> (Option<&'a str>, bool) {
    let tab_id = facts
        .get(pane)
        .map(|f| f.tab_id.as_str())
        .unwrap_or_default();
    if tab_id.is_empty() {
        return (None, false);
    }
    let tab = tabs.get(tab_id).map(|t| t.as_str());
    let multi = census.get(tab_id).copied().unwrap_or(0) > 1;
    (tab, multi)
}

/// Tab-derived naming core (`Some`) or tag-fallback (`None`): the single
/// source for watchdog, adopt, and reset naming, so the three can never
/// drift again. `None` means "format the tag unconditionally" — the
/// watchdog never preserves a bare tag, and neither does reset.
pub fn naming_core(tab: Option<&str>, tag: &str, multi: bool) -> Option<String> {
    let t = tab.map(str::trim).filter(|t| !t.is_empty())?;
    Some(if multi {
        format!("{t} {tag}")
    } else {
        t.to_string()
    })
}

/// Watchdog-parity core: labeled splits use the pane label, else tab/tag.
/// Single source for watchdog, reset, and inspect so they never drift.
pub fn title_core_for(
    tab: Option<&str>,
    tag: &str,
    multi: bool,
    pane: Option<&str>,
) -> Option<String> {
    if multi && let Some(pl) = pane.map(str::trim).filter(|l| !l.is_empty()) {
        return Some(pl.to_string());
    }
    naming_core(tab, tag, multi)
}

/// Verbatim-preservation predicate: a stored topic title that already
/// equals the herdr tab name (trim-compared) means a Telegram native
/// rename just synced both sides — the watchdog must keep it exactly,
/// never reformat. Pure so it is unit-tested, not just eyeballed.
pub fn stored_matches_label(stored: Option<&str>, label: &str) -> bool {
    stored.map(str::trim) == Some(label.trim())
}

/// Chrome-tolerant keep predicate for split panes (stored raw vs herdr
/// pane label): stored is the last-visible Telegram title, so shed
/// Format-B chrome via `topic_core` before comparing to the label.
/// Comparison is whitespace-collapsed + case-blind (`[TG] API · O`
/// covers `api`), and `topic_core` sheds ANY pasted code (stale or
/// current), so kind flips preserve the custom instead of discarding it.
pub fn stored_covers_label(stored: Option<&str>, space: &str, kind: &str, label: &str) -> bool {
    stored
        .map(|s| norm_title(&crate::topics::names::topic_core(s, space, kind)) == norm_title(label))
        .unwrap_or(false)
}

/// Kind-flip bypass: seen kind differing from current forces reformat
/// (verbatim keeps must not hide an agent→shell icon change).
pub fn kind_bypass(last: Option<&str>, cur: &str) -> bool {
    matches!(last, Some(l) if l != cur)
}

/// Pane core after a `[new-space]` rename: `None` when the remainder is
/// blank (space-only rename — keep the herdr core). Otherwise the
/// remainder shed of chrome against BOTH spaces (old suffixes like `·
/// tg` shed via the old pass, head-word echoes collapse via the new),
/// so the result round-trips through `format_title(new_space, core)`.
pub fn space_rename_core(
    rest: &str,
    old_space: &str,
    new_space: &str,
    kind: &str,
) -> Option<String> {
    if rest.trim().is_empty() {
        return None;
    }
    let via_old = crate::topics::names::topic_core(rest, old_space, kind);
    Some(crate::topics::names::topic_core(&via_old, new_space, kind))
}

/// True when another workspace already carries `new_space` (tolerant):
/// renaming into it would duplicate space names and break the 1:1
/// `[space] label` mapping — caller refuses fail-closed.
pub fn space_label_taken(spaces: &[WorkspaceInfo], ws_id: &str, new_space: &str) -> bool {
    let want = norm_title(new_space);
    spaces
        .iter()
        .any(|w| w.id != ws_id && norm_title(&w.label) == want)
}

/// Reset title decision (single call-site for every reset loop, so the
/// predicate can never drift): 1:1 Format-B always — even a pre-reset
/// stored title equal to the tab core re-renders bare with the space
/// wrap (user text preserved; kind lives in the icon). Split tabs
/// re-wrap a stored title covering the pane label (custom preserved —
/// never the tab core, which would orphan it). Pure so unit-tested.
pub fn reset_desired_title(
    pre: Option<&str>,
    space: &str,
    label: &str,
    kind: &str,
    multi: bool,
    pane_label: Option<&str>,
) -> String {
    if !multi && stored_matches_label(pre, label) {
        crate::topics::names::format_title(space, label, kind)
    } else if multi
        && let Some(pl) = pane_label.map(str::trim).filter(|l| !l.is_empty())
        && stored_covers_label(pre, space, kind, pl)
    {
        crate::topics::names::format_title(space, pl, kind)
    } else {
        crate::topics::names::format_title(space, label, kind)
    }
}

#[cfg(test)]
#[path = "title_rules_tests.rs"]
mod tests;
