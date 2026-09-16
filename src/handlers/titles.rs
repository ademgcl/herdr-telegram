//! 1:1 pane↔topic title sync. herdr→telegram runs on the reconcile
//! watchdog; telegram→herdr fires on native `forum_topic_edited`
//! updates. Both sides compare against the stored title first, so edits
//! converge instead of echo-looping. Unlabeled panes get the friendly
//! default (`[{space}] {tag} · {code}`, shells bare `[{space}] {tag}`)
//! written into their herdr label,
//! so the default name is herdr-tracked and readable — never a bare pane id.
use crate::{
    herdr::{
        client::{list_agents, list_workspaces},
        labels::{pane_facts, rename_pane},
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

/// Watchdog half: every mapped live pane's topic shows its herdr label.
/// Unlabeled panes are provisioned with the friendly default first.
/// Panes gone from herdr are skipped — the close flow owns them.
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
    sync_titles_with(s, &agents, &spaces, &facts).await;
}

/// Cached variant: reuses the reconcile tick's agents/spaces/facts so
/// the watchdog costs 0 extra list RPCs (1 `list_panes` per tick, not 8).
pub async fn sync_titles_with(
    s: &AppState,
    agents: &[crate::types::AgentRow],
    spaces: &[crate::types::WorkspaceInfo],
    facts: &std::collections::HashMap<String, crate::herdr::labels::PaneFacts>,
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
    for pane in s.topics.all_mappings().keys() {
        let Some(f) = facts.get(pane) else { continue };
        let kind = kind_of.get(pane.as_str()).copied().unwrap_or("shell");
        let space = ws_label(&spaces, &f.ws);
        if let Some(label) = f.label.as_deref().filter(|l| !l.trim().is_empty()) {
            // Herdr name wins (formatted with workspace prefix and agent tag).
            let formatted = names::format_title(space, label, kind);
            s.topics.sync_title(pane, &formatted).await;
            continue;
        }
        let tag = s.topics.tag_for(pane, kind);
        let friendly = names::title(&tag, space, kind);
        if rename_pane(&s.cfg.socket, pane, Some(&friendly))
            .await
            .is_ok()
        {
            s.topics.sync_title(pane, &friendly).await;
        }
    }
    // One liveness probe per tick: human-deleted topics never fire a
    // rename (converged titles stay quiet), so without this the mapping
    // would dangle until a rename came due.
    s.topics.probe_deleted().await;
}

/// Native topic rename → herdr pane label. Unmapped threads (General)
/// are ignored; our own sync echoes match the stored title and skip.
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
}
