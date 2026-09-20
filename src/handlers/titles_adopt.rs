//! Telegram→herdr rename adopt (split from `titles`: 300-line file limit).
//! Native `forum_topic_edited` updates rename the herdr tab (single-pane)
//! or pane label (split tab). See `titles` for the watchdog half and the
//! 1:1 format contract.
use crate::{
    handlers::title_rules::{stored_covers_label, stored_matches_label},
    herdr::{
        client::{list_agents, list_workspaces},
        labels::{pane_facts, rename_pane, rename_tab, tab_labels},
    },
    state::AppState,
    topics::names,
    ui::ws_label,
};

/// Shared rename-failure text (fail-closed adopt paths; also the
/// space-adopt arm — single source, never dup'd).
pub(crate) const STATE_READ_ERR: &str = "⚠️ rename failed: could not read herdr state — try again";

/// True when herdr shows a third core (neither the stored sync nor the
/// incoming rename): a newer herdr-side rename won the race, so the
/// redelivered Telegram edit must not revert it. Pure + tested.
fn herdr_moved_on(
    stored: Option<&str>,
    space: &str,
    kind: &str,
    incoming: &str,
    herdr_now: &str,
) -> bool {
    // No stored baseline → nothing to protect, never moved-on.
    stored.is_some()
        && !herdr_now.trim().is_empty()
        && herdr_now.trim() != incoming.trim()
        && !stored_covers_label(stored, space, kind, herdr_now.trim())
}

/// Native topic rename → herdr tab name (single-pane) or pane label
/// (split tab). Unmapped threads (General) are ignored; our own sync
/// echoes match the stored title and skip.
pub async fn adopt_topic_title(s: AppState, chat: i64, thread: Option<i64>, name: &str) {
    // Reset owns migration: a native rename mid-reset would mutate herdr
    // during the read-only window and fight re-sync.
    if crate::handlers::reset::is_resetting() {
        s.tg.send_msg(
            chat,
            thread,
            "⚠️ reset in progress — rename again post-reset",
            None,
        )
        .await;
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
            // Row-miss is shell only with confirmation (watchdog parity
            // in `titles::sync_titles_with`): a transient `agent.list`
            // dropout over a live agent must refuse, never write herdr
            // under the wrong kind's chrome rules — boot included.
            let kind = match agents.iter().find(|a| a.pane == pane) {
                Some(a) => a.kind.clone(),
                None if !super::shell::confirmed_shell(&s, &pane).await => {
                    s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
                    return;
                }
                None => "shell".to_string(),
            };
            // Bracket names the space: `[new] label` renames the
            // workspace (pane keeps `label`), never the tab to
            // `[new] label` (that duplication was the bug).
            if let Some(fm) = facts.as_ref()
                && super::titles_space::try_adopt_space_rename(
                    &s, chat, th, &pane, name, fm, &spaces, ws, &space, &kind, multi, &tab_id,
                )
                .await
            {
                return;
            }
            let core = names::topic_core(name, &space, &kind);
            if core.trim().is_empty() {
                s.tg.send_msg(
                    chat,
                    thread,
                    "⚠️ rename ignored: empty after stripping formatting",
                    None,
                )
                .await;
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
    let stored = s.topics.topic_title(&pane);
    if stored_covers_label(stored.as_deref(), &space, &kind, &core) {
        return;
    }
    // Stale-replay revert guard: herdr showing a THIRD core (covered by
    // neither stored nor incoming) means a newer herdr-side rename won
    // the race — a redelivered Telegram edit must not revert it.
    // Multi reads the pane label from facts (no RPC); single needs one
    // tab.list read (renames are rare, so the cost lands nowhere hot).
    // Fail-closed: an unreadable tab list aborts, never renames blind.
    {
        let herdr_now: Option<String> = if multi {
            facts.as_ref().and_then(|m| m.get(&pane)).and_then(|f| {
                let l = f.label.as_deref().unwrap_or("").trim();
                if l.is_empty() {
                    None
                } else {
                    Some(l.to_string())
                }
            })
        } else {
            match tab_labels(&s.cfg.socket).await {
                Ok(tabs_now) => tabs_now.get(&tab_id).cloned(),
                Err(_) => {
                    s.tg.send_msg(chat, thread, STATE_READ_ERR, None).await;
                    return;
                }
            }
        };
        if let Some(hn) = herdr_now
            && herdr_moved_on(stored.as_deref(), &space, &kind, &core, &hn)
        {
            println!("[titles] stale rename dropped for {pane} (herdr moved on)");
            return;
        }
    }
    // Tab-less single pane has no tab to rename; a pane-label write
    // would be reverted next tick (watchdog reads tabs). Fail-closed.
    if !multi && tab_id.is_empty() {
        s.tg.send_msg(
            chat,
            thread,
            "⚠️ rename failed: pane has no tab yet — try again",
            None,
        )
        .await;
        return;
    }
    // 1:1 prompt converge: after herdr takes the core, re-assert the
    // formatted title immediately (space wrap preserved) instead
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
                if let Some(p) = s.topics.sync_title(&pane, &formatted).await {
                    crate::handlers::dialog::retire_dialog(&s, &p).await;
                }
                println!("[titles] rename #{th} → tab {tab_id} ({pane}) {core:?}");
            }
            Err(e) => {
                s.tg.send_msg(
                    chat,
                    Some(th),
                    &format!(
                        "⚠️ rename failed: {}",
                        crate::types::mask_home(&e.to_string())
                    ),
                    None,
                )
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
            if let Some(p) = s.topics.sync_title(&pane, &formatted).await {
                crate::handlers::dialog::retire_dialog(&s, &p).await;
            }
            println!("[titles] rename #{th} → pane {pane} label {core:?}");
        }
        Err(e) => {
            s.tg.send_msg(
                chat,
                Some(th),
                &format!(
                    "⚠️ rename failed: {}",
                    crate::types::mask_home(&e.to_string())
                ),
                None,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_herdr_moved_on_third_core_only() {
        let stored = Some("[tg] main · o");
        // herdr == stored, incoming fresh → not moved-on.
        assert!(!herdr_moved_on(stored, "tg", "opencode", "beta", "main"));
        // herdr == incoming → idempotent, not moved-on.
        assert!(!herdr_moved_on(stored, "tg", "opencode", "beta", "beta"));
        // herdr shows a third core → moved-on, drop the redelivery.
        assert!(herdr_moved_on(stored, "tg", "opencode", "beta", "delta"));
        // Empty herdr read → unknown, never moved-on.
        assert!(!herdr_moved_on(stored, "tg", "opencode", "beta", "  "));
        assert!(!herdr_moved_on(None, "tg", "opencode", "beta", "delta"));
    }
}
