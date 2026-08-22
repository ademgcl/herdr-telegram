use serde_json::json;
use crate::{
    herdr::client::{get_agent, list_workspaces},
    state::AppState,
    ui::{agent_topic_action_kb, btn, emoji, ws_label},
};

/// Force-refresh an agent's forum topic title to reflect `status`
/// (used by the prompt pipeline for instant 🔄 / settled flips).
pub async fn refresh_topic_title(s: &AppState, pane: &str, status: &str) {
    let Ok(a) = get_agent(&s.cfg.socket, pane).await else { return };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    s.topics.update_topic_title(pane, &a.kind, ws_label(&spaces, &a.ws), status).await;
}

pub async fn observe_status(
    s: &AppState,
    pane: &str,
    new_status: &str,
    silent: bool,
    src: &str,
) {
    let old = {
        let mut m = s.status.lock().await;
        m.insert(pane.to_string(), new_status.to_string())
    };

    if silent || old.as_deref() == Some(new_status) {
        return;
    }

    let is_attention = matches!(new_status, "blocked" | "done" | "idle");
    if !is_attention {
        return;
    }

    // Collapse rapid done <-> idle flap
    if (old.as_deref() == Some("done") && new_status == "idle")
        || (old.as_deref() == Some("idle") && new_status == "done")
    {
        println!("[alert] collapsed {old:?}→{new_status} for {pane} ({src})");
        return;
    }

    // Suppress parallel alert if active prompt job is running
    if s.jobs.lock().await.contains_key(pane) {
        return;
    }

    println!("[alert] {src}: {pane} {old:?}→{new_status}");

    let info = get_agent(&s.cfg.socket, pane).await.ok();
    let (kind, ws_id, title) = match &info {
        Some(a) => (a.kind.clone(), a.ws.clone(), a.title.clone()),
        None => ("?".into(), "?".into(), String::new()),
    };

    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let raw_space = ws_label(&spaces, &ws_id);
    let space_label = spaces
        .iter()
        .find(|w| w.id == ws_id)
        .map(|w| format!("#{} {}", w.number, w.label))
        .unwrap_or_else(|| ws_id.clone());

    let hint = match new_status {
        "blocked" => "\n↩️ reply or type in topic to answer",
        _ => "",
    };
    let verb = if new_status == "idle" { "ready" } else { new_status };
    let mut text = format!("{} {}: {kind} @ {space_label}", emoji(new_status), verb);
    if !title.is_empty() {
        let short: String = title.chars().take(60).collect();
        text.push_str(&format!("\n{short}"));
    }
    text.push_str(hint);

    s.set_focus(pane).await;

    // Send to agent forum topic if configured; flag topic unread + show it in title
    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.ensure_topic(pane, &kind, raw_space, new_status).await {
            let mid = s.tg
                .send_msg(forum, Some(thread), &text, Some(agent_topic_action_kb(pane)))
                .await;
            s.remember(forum, mid, pane).await;
            if mid.is_some() && s.topics.mark_unread(pane) {
                s.topics.update_topic_title(pane, &kind, raw_space, new_status).await;
            }
        }
    }

    // Also send alert to DM owners
    for id in &s.cfg.owners {
        let mid = s.tg
            .send_msg(
                *id,
                None,
                &text,
                Some(json!([[btn("show output", &format!("o:{pane}"))]])),
            )
            .await;
        s.remember(*id, mid, pane).await;
    }
}
