//! 1:1 tab↔topic title sync. herdr→telegram runs on the reconcile
//! watchdog; telegram→herdr fires on native `forum_topic_edited`
//! updates. Both sides compare against the stored title first, so edits
//! converge instead of echo-looping. Title source is the user-visible
//! herdr TAB name (`tab.rename`/`tab.list`) — never the terminal/agent
//! title (that lives only in the identity card). Tab missing/empty falls
//! back to the stable tag (`[{space}] {tag} · {code}`, e.g. `[tg] o2 · o`).
//! Pane labels are written only for split-tab user renames (a shared tab
//! can't disambiguate); the watchdog formats everything else from the
//! tab core, preserving user-set names verbatim on both paths.
//! Telegram→herdr renames shed Format-B chrome first (`[space]` prefix,
//! `· agent` suffix via [`names::topic_core`]): users edit the rendered
//! title, and herdr already shows the space — only the bare core is
//! written. Both paths store the raw text (stored==visible, so the
//! probe re-asserts what Telegram shows): single-pane bare renames
//! verbatim-keep, chrome pastes converge via one format rename;
//! split-pane keeps chrome-tolerantly via `stored_covers_label`.
use crate::{
    handlers::title_rules::{
        naming_core, stored_covers_label, stored_matches_label, tab_census, tab_of,
    },
    herdr::{
        client::{list_agents, list_workspaces},
        labels::{pane_facts, rename_pane, rename_tab, tab_labels},
    },
    state::AppState,
    topics::names,
    ui::ws_label,
};
use serde_json::Value;
use std::collections::HashMap;

/// Shared rename-failure text (fail-closed adopt paths).
const STATE_READ_ERR: &str = "⚠️ rename failed: could not read herdr state — try again";

/// Pure extract of a native topic rename: (thread, new name). Service
/// messages carry no text, so the router must branch on this BEFORE its
/// empty-text return (that ordering bug ate every rename once already).
pub fn parse_topic_edit(msg: &Value) -> Option<(i64, String)> {
    msg.get("forum_topic_edited")?;
    let thread = msg["message_thread_id"].as_i64()?;
    let name = msg["forum_topic_edited"]["name"]
        .as_str()?
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    Some((thread, name))
}

/// Extract user-edited topic icon custom-emoji ID from a `forum_topic_edited` service msg.
pub fn parse_topic_icon_edit(msg: &Value) -> Option<(i64, String)> {
    msg.get("forum_topic_edited")?;
    let thread = msg["message_thread_id"].as_i64()?;
    let icon = msg["forum_topic_edited"]["icon_custom_emoji_id"]
        .as_str()?
        .trim()
        .to_string();
    if icon.is_empty() {
        return None;
    }
    Some((thread, icon))
}

/// Watchdog half: every mapped live pane's topic shows its herdr TAB
/// name (or the tag default when the tab has none). Panes gone from
/// herdr are skipped — the close flow owns them.
/// Prefer `sync_titles_with` on the hot path (reuses the tick's fetch).
#[allow(dead_code)]
pub async fn sync_titles(s: &AppState) {
    // Reset owns Steps 1-4: the watchdog must not rename topics about
    // to die (or mint through the gate) mid-reset. Reset's Step 4 calls
    // `sync_title` directly, so it is unaffected.
    if crate::handlers::reset::is_resetting() {
        return;
    }
    if s.cfg.forum.is_none() || s.topics.all_mappings().is_empty() {
        return;
    }
    // Fail-closed inputs: a degraded fetch must skip the tick, never
    // reformat every topic from tag/? defaults (mass-revert on outage).
    let Ok(facts) = pane_facts(&s.cfg.socket).await else {
        return;
    };
    let Ok(agents) = list_agents(&s.cfg.socket).await else {
        return;
    };
    let Ok(spaces) = list_workspaces(&s.cfg.socket).await else {
        return;
    };
    let Ok(tabs) = tab_labels(&s.cfg.socket).await else {
        return;
    };
    sync_titles_with(s, &agents, &spaces, &facts, &tabs).await;
}

/// Cached variant: reuses the reconcile tick's agents/spaces/facts/tabs
/// so the watchdog costs 1 extra list RPC (tab.list) per tick, not 8.
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
        let kind = kind_of.get(pane.as_str()).copied().unwrap_or("shell");
        let kind_flip = s.topics.kind_changed(pane, kind);
        s.topics.note_kind(pane, kind);
        let space = ws_label(spaces, &f.ws);
        let (tab, multi) = tab_of(facts, tabs, &census, pane);
        // Verbatim preservation (merged upstream): a stored title equal to
        // the tab name means a Telegram native rename just synced both
        // sides — keep it exactly, never reformat (single-pane; the
        // split pane-label rule follows right below).
        if !multi
            && !kind_flip
            && let Some(t) = tab
            && stored_matches_label(s.topics.topic_title(pane).as_deref(), t)
        {
            continue;
        }
        // Split tabs share one tab label, so pane-level renames live in
        // the pane label: a stored title equal to the pane's herdr label
        // is user-set — keep it exactly like single-pane verbatim above,
        // or the format below reverts it every tick.
        if multi
            && !kind_flip
            && let Some(pl) = f.label.as_deref()
            && !pl.trim().is_empty()
            && stored_covers_label(s.topics.topic_title(pane).as_deref(), space, kind, pl)
        {
            continue;
        }
        let tag = s.topics.tag_for(pane, kind);
        // One naming source for watchdog and reset (`naming_core`): the
        // two can never drift again. None → tag default, which the
        // watchdog never preserves verbatim.
        let pane_label = f.label.as_deref().filter(|l| !l.trim().is_empty());
        let mut core = naming_core(tab, &tag, multi);
        // Labeled splits render the pane label (flips re-suffix, relabels converge).
        if multi && let Some(pl) = pane_label {
            core = Some(pl.to_string());
        }
        let formatted = names::format_title(space, core.as_deref().unwrap_or(&tag), kind);
        s.topics.sync_title(pane, &formatted).await;
    }
    // One liveness probe per tick: human-deleted topics never fire a
    // rename (converged titles stay quiet), so without this the mapping
    // would dangle until a rename came due.
    s.topics.probe_deleted().await;
}

/// Native topic rename → herdr tab name (single-pane) or pane label
/// (split tab). Unmapped threads (General) are ignored; our own sync
/// echoes match the stored title and skip.
pub async fn adopt_topic_title(s: AppState, chat: i64, thread: Option<i64>, name: &str) {
    // Reset owns migration: a native rename mid-reset would mutate herdr
    // during the read-only window and fight re-sync. Dropped:
    // reset rebuilds from herdr, re-apply post-reset.
    if crate::handlers::reset::is_resetting() {
        return;
    }
    let Some(th) = thread else { return };
    let Some(pane) = s.topics.pane_of_thread(th) else {
        println!("[titles] rename to {name:?} in unmapped thread #{th} — ignored");
        return;
    };
    let name = name.trim();
    if name.is_empty() || stored_matches_label(s.topics.topic_title(&pane).as_deref(), name) {
        return;
    }
    // Single-pane tab: the user-visible name is the tab — rename it so
    // herdr's tab bar follows Telegram. Split tabs share one tab label,
    // so rename only the pane there.
    let facts = pane_facts(&s.cfg.socket).await.ok();
    let tab_id = facts
        .as_ref()
        .and_then(|m| m.get(&pane))
        .map(|f| f.tab_id.clone())
        .unwrap_or_default();
    let shared = facts
        .as_ref()
        .map(|m| m.values().filter(|f| f.tab_id == tab_id).count())
        .unwrap_or(0);
    let multi = !tab_id.is_empty() && shared > 1;
    // Fail-closed: no facts entry = degraded read — never rename blind.
    if facts.as_ref().and_then(|m| m.get(&pane)).is_none() {
        s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
        return;
    }
    // Shed Format-B chrome users inherit from the rendered title
    // (`[space] main · opencode` → `main`): herdr shows the space
    // already, so only the bare core is written. Fail-closed: unreadable
    // kind/space aborts with ⚠️ above (never a blind chrome write).
    let (core, kind) = match (
        list_workspaces(&s.cfg.socket).await.ok(),
        list_agents(&s.cfg.socket).await.ok(),
    ) {
        (Some(spaces), Some(agents)) => {
            let ws = facts
                .as_ref()
                .and_then(|m| m.get(&pane))
                .map(|f| f.ws.as_str())
                .unwrap_or("");
            let kind = agents
                .iter()
                .find(|a| a.pane == pane)
                .map(|a| a.kind.as_str())
                .unwrap_or("shell");
            (
                names::topic_core(name, ws_label(&spaces, ws), kind),
                kind.to_string(),
            )
        }
        _ => {
            s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
            return;
        }
    };
    if !multi && !tab_id.is_empty() {
        match rename_tab(&s.cfg.socket, &tab_id, &core).await {
            Ok(()) => {
                // Same re-gate as the pane path below: a remint landing
                // mid-RPC must not gain a stale title.
                if crate::handlers::reset::is_resetting() {
                    return;
                }
                if !s.topics.note_title_if_thread(&pane, th, name) {
                    println!("[titles] stale rename dropped for {pane}");
                    return;
                }
                s.topics.note_kind(&pane, &kind);
                println!("[titles] rename #{th} → tab {tab_id} ({pane}) {core:?}");
                s.tg.send_msg(chat, thread, &format!("✏️ tab → `{core}`"), None)
                    .await;
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(th), &format!("⚠️ rename failed: {e}"), None)
                    .await;
            }
        }
        return;
    }
    match rename_pane(&s.cfg.socket, &pane, Some(core.as_str())).await {
        Ok(()) => {
            // Re-gate after the await + CAS-store: a remint (reset /
            // probe prune) landing mid-RPC must not gain a stale title —
            // plain note_title would poison the fresh mapping and the
            // watchdog would then preserve the wrong title forever.
            if crate::handlers::reset::is_resetting() {
                return;
            }
            // Stored==visible: keep the raw last-visible title so
            // probe_deleted re-asserts exactly what Telegram shows;
            // the watchdog chrome-tolerantly keeps it via
            // stored_covers_label (raw stored vs herdr pane label).
            if !s.topics.note_title_if_thread(&pane, th, name) {
                println!("[titles] stale rename dropped for {pane}");
                return;
            }
            s.topics.note_kind(&pane, &kind);
            println!("[titles] rename #{th} → pane {pane} label {core:?}");
            s.tg.send_msg(chat, thread, &format!("✏️ pane label → `{core}` (tab shared by {shared} — rename the tab on the PC to rename all)"), None).await;
        }
        Err(e) => {
            s.tg.send_msg(chat, Some(th), &format!("⚠️ rename failed: {e}"), None)
                .await;
        }
    }
}

#[cfg(test)]
#[path = "titles_tests.rs"]
mod tests;
