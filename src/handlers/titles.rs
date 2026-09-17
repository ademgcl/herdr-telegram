//! 1:1 tab↔topic title sync. herdr→telegram runs on the reconcile
//! watchdog; telegram→herdr fires on native `forum_topic_edited`
//! updates. Both sides compare against the stored title first, so edits
//! converge instead of echo-looping. Title source is the user-visible
//! herdr TAB name (`tab.rename`/`tab.list`) — never the terminal/agent
//! title (that lives only in the identity card). Tab missing/empty falls
//! back to the stable tag (`[{space}] {tag} · {code}`, e.g. `[tg] o2 · o`).
//! Pane labels are written only for split-tab user renames (a shared tab
//! can't disambiguate); the watchdog formats everything else from the
//! tab core. 1:1 Format-B always: every topic shows
//! `[space] label · code`, so a Telegram rename to `Custom` converges to
//! `[space] Custom · code` (space + agent code preserved, promptly —
//! adopt re-asserts the formatted title right after the herdr rename,
//! the watchdog converges herdr edits next tick).
//! Telegram→herdr renames shed Format-B chrome tolerantly
//! (`[space]` prefix case-blind/collapsed/truncated, any `·•⋅` code,
//! `| : / -` spaced variants, stale codes — see [`names`]): users edit
//! the rendered title, and herdr already shows the space — only the bare
//! core is written.
use crate::{
    handlers::title_rules::{stored_covers_label, stored_matches_label, tab_census, tab_of, title_core_for},
    herdr::{
        client::{list_agents, list_workspaces},
        labels::{pane_facts, rename_pane, rename_tab, tab_labels},
    },
    state::AppState,
    topics::names,
    ui::ws_label,
};
use std::collections::HashMap;

/// Shared rename-failure text (fail-closed adopt paths).
const STATE_READ_ERR: &str = "⚠️ rename failed: could not read herdr state — try again";

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
        // Degraded `list_workspaces`: an unmapped id would render as the
        // raw id (`[w8] …`) — skip the pane, never corrupt the title.
        if !spaces.iter().any(|w| w.id == f.ws) {
            continue;
        }
        // Shells are invisible to `agent.list`: a missing row is shell
        // ONLY when the last seen kind agrees — otherwise it is a
        // transient dropout, and defaulting would flip `· o` → `· sh`.
        let kind = match kind_of.get(pane.as_str()).copied() {
            Some(k) => k,
            None if s.topics.kind_changed(pane, "shell") => continue,
            None => "shell",
        };
        s.topics.note_kind(pane, kind);
        // Kind flips re-icon bot-owned topics (user customs skipped):
        // the glyph is the at-a-glance kind signal next to the short
        // suffix. RPC only on an actual flip; failures retry next tick.
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
    // during the read-only window and fight re-sync.
    if crate::handlers::reset::is_resetting() {
        s.tg.send_msg(chat, thread, "⚠️ reset in progress — rename again post-reset", None).await;
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
    // so rename only the pane there. census via shared helper would need
    // tabs; the hand-count below matches `tab_census` on non-empty ids.
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
    // (`[space] main · o` → `main`): herdr shows the space already, so
    // only the bare core is written. Fail-closed: unreadable/unmapped
    // kind/space aborts with ⚠️ (never a blind chrome write); a missing
    // agent row is shell only when the last kind agrees, else dropout.
    let (core, kind, space) = match (
        list_workspaces(&s.cfg.socket).await.ok(),
        list_agents(&s.cfg.socket).await.ok(),
    ) {
        (Some(spaces), Some(agents)) => {
            let ws = facts
                .as_ref()
                .and_then(|m| m.get(&pane))
                .map(|f| f.ws.as_str())
                .unwrap_or("");
            if !spaces.iter().any(|w| w.id == ws) {
                s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
                return;
            }
            let space = ws_label(&spaces, ws).to_string();
            let kind = match agents.iter().find(|a| a.pane == pane) {
                Some(a) => a.kind.clone(),
                None if s.topics.kind_changed(&pane, "shell") => {
                    s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
                    return;
                }
                None => "shell".to_string(),
            };
            let core = names::topic_core(name, &space, &kind);
            if core.trim().is_empty() {
                s.tg.send_msg(chat, thread, "⚠️ rename ignored: empty after stripping title chrome", None).await;
                return;
            }
            (core, kind, space)
        }
        _ => {
            s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
            return;
        }
    };
    // Redelivery/out-of-order guard: herdr already reflecting this core
    // (stored formatted covering it) means a stale replay — skip the
    // RPC + confirm spam (last-writer-wins, not first). Single and
    // split alike: `stored_covers_label` sheds chrome, so a formatted
    // stored title covers the bare core on both paths.
    if stored_covers_label(s.topics.topic_title(&pane).as_deref(), &space, &kind, &core) {
        return;
    }
    // Tab-less single pane has no tab to rename; a pane-label write
    // would be reverted next tick (watchdog reads tabs). Fail-closed.
    if !multi && tab_id.is_empty() {
        s.tg.send_msg(chat, thread, "⚠️ rename failed: pane has no tab yet — try again", None).await;
        return;
    }
    // 1:1 prompt converge: after herdr takes the core, re-assert the
    // formatted title immediately (space + fresh code preserved) instead
    // of waiting up to 60s for the watchdog.
    let formatted = names::format_title(&space, &core, &kind);
    if !multi {
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
                s.topics.sync_title(&pane, &formatted).await;
                println!("[titles] rename #{th} → tab {tab_id} ({pane}) {core:?}");
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
            // probe prune) landing mid-RPC must not gain a stale title.
            if crate::handlers::reset::is_resetting() {
                return;
            }
            if !s.topics.note_title_if_thread(&pane, th, name) {
                println!("[titles] stale rename dropped for {pane}");
                return;
            }
            s.topics.note_kind(&pane, &kind);
            s.topics.sync_title(&pane, &formatted).await;
            println!("[titles] rename #{th} → pane {pane} label {core:?}");
        }
        Err(e) => {
            s.tg.send_msg(chat, Some(th), &format!("⚠️ rename failed: {e}"), None)
                .await;
        }
    }
}
