use serde_json::json;
use crate::{
    herdr::client::{get_agent, list_workspaces},
    state::AppState,
    ui::{btn, emoji, ws_label},
};

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

    // NOTE: deliberately NOT touching focus here — background alerts must never
    // hijack where the owner's next plain-text message gets delivered.

    // Alerts belong WHERE THE AGENT LIVES: its topic, as plain chat text.
    // Direct messages are only for non-forum setups. Never both.
    if let Some(forum) = s.cfg.forum {
        if let Some(thread) = s.topics.ensure_topic(pane, &kind, raw_space).await {
            let mid = s.tg.send_msg(forum, Some(thread), &text, None).await;
            s.remember(forum, mid, pane).await;
        }
    } else {
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
}
