//! Single topic reset: deletes a single pane/topic on Telegram and recreates
//! it fresh (preserving queue messages, resetting pins and updating mappings).
use crate::{
    herdr::{
        client::{get_agent, list_workspaces},
        labels::pane_facts,
    },
    state::AppState,
    types::Res,
    ui::ws_label,
};

pub async fn run_single_topic_reset(
    s: &AppState,
    chat: i64,
    thread_id: Option<i64>,
    target: &str,
) -> Res<String> {
    let forum = s
        .cfg
        .forum
        .ok_or("Reset is only available in forum supergroup mode")?;

    // F9: permission guard
    if let Ok(perms) = s.tg.check_forum_permissions(forum).await {
        if !perms.can_manage_topics {
            let msg = "⚠️ reset aborted: bot lacks 'can_manage_topics' admin permission";
            if chat != 0 {
                s.tg.send_msg(chat, thread_id, msg, None).await;
            }
            return Err(msg.into());
        }
    }

    let clean_target = target.trim().strip_prefix('#').unwrap_or(target.trim());
    let pane = if let Ok(th) = clean_target.parse::<i64>() {
        s.topics
            .storage
            .get_pane(th)
            .ok_or_else(|| format!("no pane found for topic #{th}"))?
    } else {
        clean_target.to_string()
    };

    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let facts = pane_facts(&s.cfg.socket).await.unwrap_or_default();

    s.cancel_jobs_for(&pane).await;

    let (kind, status, raw_title) = if let Ok(agent) = get_agent(&s.cfg.socket, &pane).await {
        let title = facts
            .get(&pane)
            .and_then(|f| f.label.as_deref())
            .filter(|l| !l.trim().is_empty())
            .or_else(|| {
                if agent.title.trim().is_empty() {
                    None
                } else {
                    Some(agent.title.as_str())
                }
            });
        (agent.kind, agent.status, title.map(|t| t.to_string()))
    } else {
        let title = facts
            .get(&pane)
            .and_then(|f| f.label.as_deref())
            .filter(|l| !l.trim().is_empty());
        ("shell".to_string(), "ready".to_string(), title.map(|t| t.to_string()))
    };

    let ws = facts.get(&pane).map(|f| f.ws.as_str()).unwrap_or("");
    let space = ws_label(&spaces, ws);

    let old_thread = s.topics.storage.get_thread(&pane);
    match s
        .topics
        .reset_topic(&pane, &kind, space, &status, raw_title.as_deref(), None)
        .await
    {
        Some(new_th) => {
            let old_desc = old_thread
                .map(|t| format!("#{t}"))
                .unwrap_or_else(|| "none".into());
            let msg = format!("✅ Reset topic for {pane} ({kind}): old {old_desc} → new #{new_th}");
            if chat != 0 {
                s.tg.send_msg(chat, thread_id, &msg, None).await;
            }
            Ok(msg)
        }
        None => {
            let msg = format!("⚠️ Failed to reset topic for {pane}");
            if chat != 0 {
                s.tg.send_msg(chat, thread_id, &msg, None).await;
            }
            Err(msg.into())
        }
    }
}
