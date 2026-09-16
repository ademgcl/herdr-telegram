//! 1:1 tab↔topic title sync. herdr→telegram runs on the reconcile
//! watchdog; telegram→herdr fires on native `forum_topic_edited`
//! updates. Both sides compare against the stored title first, so edits
//! converge instead of echo-looping. Title source is the user-visible
//! herdr TAB name (`tab.rename`/`tab.list`) — never the terminal/agent
//! title (that lives only in the pinned card). Tab missing/empty falls
//! back to the stable tag (`[{space}] {tag} · {agent}`, shells bare).
//! Nothing is ever written to pane labels: the old pane-label path is
//! removed, so bot-generated names can't pollute herdr again.
use crate::{
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
    let Ok(facts) = pane_facts(&s.cfg.socket).await else {
        return;
    };
    let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let tabs = tab_labels(&s.cfg.socket).await.unwrap_or_default();
    sync_titles_with(s, &agents, &spaces, &facts, &tabs).await;
}

/// Pick the bare core for a topic title: user-visible tab name, else
/// the stable tag. Terminal/agent titles are NEVER used here — they live
/// only in the pinned identity card. Multi-pane tabs disambiguate with
/// the tag (`console` + `o27` → `console o27`), so split siblings never
/// collide. Empty/whitespace names count as missing (herdr tab names are
/// never blank in practice — this is just the safety net).
pub fn pick_core(tab: Option<&str>, tag: &str, multi: bool) -> String {
    if let Some(t) = tab.map(str::trim).filter(|t| !t.is_empty()) {
        if multi {
            return format!("{t} {tag}");
        }
        return t.to_string();
    }
    tag.to_string()
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
    let kind_of: HashMap<&str, &str> = agents
        .iter()
        .map(|a| (a.pane.as_str(), a.kind.as_str()))
        .collect();
    // Split-tab guard: siblings sharing one tab_id disambiguate with tag.
    let mut tab_count: HashMap<&str, usize> = HashMap::new();
    for f in facts.values() {
        if !f.tab_id.is_empty() {
            *tab_count.entry(f.tab_id.as_str()).or_default() += 1;
        }
    }
    for pane in s.topics.all_mappings().keys() {
        let Some(f) = facts.get(pane) else { continue };
        let kind = kind_of.get(pane.as_str()).copied().unwrap_or("shell");
        let space = ws_label(&spaces, &f.ws);
        let tab = if f.tab_id.is_empty() {
            None
        } else {
            tabs.get(f.tab_id.as_str()).map(|t| t.as_str())
        };
        let multi = !f.tab_id.is_empty() && tab_count.get(f.tab_id.as_str()).copied().unwrap_or(0) > 1;
        let tag = s.topics.tag_for(pane, kind);
        let core = pick_core(tab, &tag, multi);
        let formatted = names::format_title(space, &core, kind);
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
    let Some(th) = thread else { return };
    let Some(pane) = s.topics.pane_of_thread(th) else {
        println!("[titles] rename to {name:?} in unmapped thread #{th} — ignored");
        return;
    };
    let name = name.trim();
    if name.is_empty() || s.topics.topic_title(&pane).as_deref() == Some(name) {
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
    let multi = !tab_id.is_empty()
        && facts
            .as_ref()
            .map(|m| m.values().filter(|f| f.tab_id == tab_id).count() > 1)
            .unwrap_or(false);
    if !multi && !tab_id.is_empty() {
        match rename_tab(&s.cfg.socket, &tab_id, name).await {
            Ok(()) => {
                s.topics.note_title(&pane, name);
                println!("[titles] topic #{th} renamed → tab {tab_id} ({pane}) {name:?}");
            }
            Err(e) => {
                s.tg.send_msg(chat, Some(th), &format!("⚠️ rename failed: {e}"), None)
                    .await;
            }
        }
        return;
    }
    match rename_pane(&s.cfg.socket, &pane, Some(name)).await {
        Ok(()) => {
            s.topics.note_title(&pane, name);
            println!("[titles] topic #{th} renamed → pane {pane} label {name:?}");
        }
        Err(e) => {
            s.tg.send_msg(chat, Some(th), &format!("⚠️ rename failed: {e}"), None)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit_msg(thread: Option<i64>, name: &str) -> Value {
        let mut m = json!({"message_thread_id": 17, "forum_topic_edited": {"name": name}});
        if let Some(th) = thread {
            m["message_thread_id"] = json!(th);
        } else {
            m.as_object_mut().unwrap().remove("message_thread_id");
        }
        m
    }

    #[test]
    fn test_parse_topic_edit_service_msg() {
        assert_eq!(
            parse_topic_edit(&edit_msg(Some(17), "o2 · myblender")),
            Some((17, "o2 · myblender".to_string()))
        );
        // Padded names trim.
        assert_eq!(
            parse_topic_edit(&edit_msg(Some(17), "  api  ")),
            Some((17, "api".to_string()))
        );
    }

    #[test]
    fn test_parse_topic_edit_rejects_non_edits() {
        // Plain text message: no forum_topic_edited key.
        assert_eq!(parse_topic_edit(&json!({"text": "/space x"})), None);
        // Missing thread or blank name: unroutable.
        assert_eq!(parse_topic_edit(&edit_msg(None, "api")), None);
        assert_eq!(parse_topic_edit(&edit_msg(Some(17), "   ")), None);
        assert_eq!(
            parse_topic_edit(&json!({"message_thread_id": 17, "forum_topic_edited": {}})),
            None
        );
    }

    #[test]
    fn test_parse_topic_icon_edit() {
        let msg = json!({
            "message_thread_id": 17,
            "forum_topic_edited": {"icon_custom_emoji_id": "5350554349074391003"}
        });
        assert_eq!(
            parse_topic_icon_edit(&msg),
            Some((17, "5350554349074391003".to_string()))
        );
        assert_eq!(
            parse_topic_icon_edit(
                &json!({"message_thread_id": 17, "forum_topic_edited": {"name": "hi"}})
            ),
            None
        );
    }

    #[test]
    fn test_pick_core_prefers_tab() {
        // User-visible tab name wins; empty/missing falls back to tag.
        // Terminal titles never reach here (pinned card only).
        assert_eq!(pick_core(Some("agy_gelistirme"), "a14", false), "agy_gelistirme");
        assert_eq!(pick_core(Some("console"), "o27", false), "console");
        // Split tabs disambiguate with the tag.
        assert_eq!(pick_core(Some("console"), "o27", true), "console o27");
        // No tab: stable tag default. Blank counts as missing.
        assert_eq!(pick_core(None, "o1", false), "o1");
        assert_eq!(pick_core(Some("  "), "sh1", false), "sh1");
    }
}
