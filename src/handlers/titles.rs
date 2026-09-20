//! 1:1 tab↔topic title sync. herdr→telegram runs on the reconcile
//! watchdog; telegram→herdr fires on native `forum_topic_edited`
//! updates. Both sides compare against the stored title first, so edits
//! converge instead of echo-looping. Title source is the user-visible
//! herdr TAB name (`tab.rename`/`tab.list`) — never the terminal/agent
//! title (that lives only in the identity card). Tab missing/empty falls
//! back to the stable tag (`[{space}] {tag}`, e.g. `[tg] o2`).
//! Pane labels are written only for split-tab user renames (a shared tab
//! can't disambiguate); the watchdog formats everything else from the
//! tab core. 1:1 Format-B always: every topic shows `[space] label`
//! (kind lives in the icon), so a Telegram rename to `Custom` converges
//! to `[space] Custom` promptly — adopt re-asserts the formatted title
//! right after the herdr rename, the watchdog converges herdr edits
//! next tick.
//! Telegram→herdr renames shed Format-B chrome tolerantly
//! (`[space]` prefix case-blind/collapsed/truncated, any `·•⋅` code,
//! `| : / -` spaced variants, stale codes — see [`names`]): users edit
//! the rendered title, and herdr already shows the space — only the bare
//! core is written. A leading `[different]` bracket instead renames the
//! SPACE (see `titles_space`) — never the tab (no `[old] [new]` dupes).
use crate::{
    handlers::title_rules::{tab_census, tab_of, title_core_for},
    state::AppState,
    topics::names,
    ui::ws_label,
};
use std::collections::HashMap;

/// Partial-read guard (pure, tested): tab_id present but missing from
/// a non-empty tab map is a degraded `tab.list` (not a deleted tab) —
/// the caller skips the pane instead of mass-reverting to the tag
/// default. Empty map is the hollow-list case handled above, never
/// partial.
pub(crate) fn tab_read_partial(tab_id: &str, tabs: &HashMap<String, String>) -> bool {
    !tab_id.is_empty() && !tabs.is_empty() && !tabs.contains_key(tab_id)
}

/// Watchdog half: every mapped live pane's topic shows its herdr TAB
/// name (or the tag default when the tab has none). Panes gone from
/// herdr are skipped — the close flow owns them. Reuses the reconcile
/// tick's agents/spaces/facts/tabs, so the watchdog costs 1 extra list
/// RPC (tab.list) per tick, not 8.
pub async fn sync_titles_with(
    s: &AppState,
    agents: &[crate::types::AgentRow],
    spaces: &[crate::types::WorkspaceInfo],
    facts: &std::collections::HashMap<String, crate::herdr::labels::PaneFacts>,
    tabs: &std::collections::HashMap<String, String>,
) {
    if crate::handlers::reset::is_resetting() {
        return;
    }
    if s.cfg.forum.is_none() || s.topics.all_mappings().is_empty() {
        return;
    }
    // Hollow-list guard: an Ok-but-empty tabs/spaces map while panes
    // claim tabs (or agents carry workspaces) is a degraded read, not a
    // real empty — skipping beats mass-reverting to tag/? defaults.
    // (Err already skips at both call sites.)
    if tabs.is_empty() && facts.values().any(|f| !f.tab_id.is_empty()) {
        return;
    }
    if spaces.is_empty() && agents.iter().any(|a| !a.ws.is_empty()) {
        return;
    }
    let kind_of: HashMap<&str, &str> = agents
        .iter()
        .map(|a| (a.pane.as_str(), a.kind.as_str()))
        .collect();
    // Split-tab guard: siblings sharing one tab_id disambiguate with tag.
    let census = tab_census(facts);
    for pane in s.topics.all_mappings().keys() {
        let Some(f) = facts.get(pane) else { continue };
        if tab_read_partial(&f.tab_id, tabs) {
            continue;
        }
        // Degraded `list_workspaces`: an unmapped id would render as the
        // raw id (`[w8] …`) — skip the pane, never corrupt the title.
        if !spaces.iter().any(|w| w.id == f.ws) {
            continue;
        }
        // Shells are invisible to `agent.list`: a missing row is shell
        // ONLY when status/tag already says so (a transient dropout must
        // never mis-mark the icon — boot included, last==None is no
        // proof) — and then it proceeds, so the flip converges instead
        // of skipping every tick forever.
        let kind = match kind_of.get(pane.as_str()).copied() {
            Some(k) => k,
            None if !super::shell::confirmed_shell(s, pane).await => {
                continue;
            }
            None => "shell",
        };
        s.topics.note_kind(pane, kind);
        // Kind flips re-icon bot-owned topics (user customs skipped):
        // the glyph is the at-a-glance kind signal (titles stay bare).
        // RPC only on an actual flip; failures retry next tick.
        if let Some(want) =
            names::icon_needs_update(s.topics.storage.get_icon(pane).as_deref(), kind)
            && let (Some(forum), Some(thread)) =
                (s.cfg.forum, s.topics.all_mappings().get(pane).copied())
            && s.tg.set_topic_icon(forum, thread, want).await.is_ok()
        {
            s.topics.storage.set_icon(pane, want);
        }
        let space = ws_label(spaces, &f.ws);
        let (tab, multi) = tab_of(facts, tabs, &census, pane);
        let tag = s.topics.tag_for(pane, kind);
        // One naming source for watchdog and reset (`title_core_for`):
        // the two can never drift again. None → tag default, which the
        // watchdog never preserves verbatim.
        let pane_label = f.label.as_deref().filter(|l| !l.trim().is_empty());
        let core = title_core_for(tab, &tag, multi, pane_label);
        let formatted = names::format_title(space, core.as_deref().unwrap_or(&tag), kind);
        if let Some(p) = s.topics.sync_title(pane, &formatted).await {
            crate::handlers::dialog::retire_dialog(s, &p).await;
        }
    }
    // Scaled liveness probe per tick: human-deleted topics never fire a
    // rename (converged titles stay quiet), so without this the mapping
    // would dangle until a rename came due. Pruned panes retire their
    // dialog generation (same stale-sig silence as a reset remint).
    for p in s.topics.probe_deleted().await {
        crate::handlers::dialog::retire_dialog(s, &p).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_tab_read_partial_skips_degraded_only() {
        // Tab id missing from a NON-empty map: degraded tab.list read —
        // skip the pane instead of reverting to the tag default.
        let tabs = HashMap::from([("t1".to_string(), "Tab".to_string())]);
        assert!(tab_read_partial("t9", &tabs));
        // Present id, empty id, and hollow (empty) map: never partial.
        assert!(!tab_read_partial("t1", &tabs));
        assert!(!tab_read_partial("", &tabs));
        assert!(!tab_read_partial("t9", &HashMap::new()));
        assert!(!tab_read_partial("", &HashMap::new()));
    }
}
