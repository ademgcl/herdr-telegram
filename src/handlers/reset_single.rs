//! Single topic reset: deletes a single pane/topic on Telegram and recreates
//! it fresh (preserving queue messages, resetting pins and updating mappings).
use crate::{
    handlers::title_rules::{naming_core, tab_census, tab_of},
    herdr::{
        client::{get_agent, list_workspaces},
        labels::{pane_facts, tab_labels},
    },
    state::AppState,
    topics::manager::ResetNames,
    types::Res,
    ui::ws_label,
};

/// Split a reset target: `#627`/`627` → thread id, anything else → pane
/// name. Pure so the (destructive) reset path's parsing is unit-tested.
pub fn split_reset_target(target: &str) -> Result<i64, String> {
    let clean = target.trim().strip_prefix('#').unwrap_or(target.trim());
    clean.parse::<i64>().map_err(|_| clean.to_string())
}

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
    if let Ok(perms) = s.tg.check_forum_permissions(forum).await
        && !perms.can_manage_topics
    {
        let msg = "⚠️ reset aborted: bot lacks 'can_manage_topics' admin permission";
        if chat != 0 {
            s.tg.send_msg(chat, thread_id, msg, None).await;
        }
        return Err(msg.into());
    }

    // Share the paced-reset lock: without it the watchdog renames the
    // topic mid-mint and a concurrent paced reset double-migrates it.
    let Some(_guard) = super::reset::try_begin_reset() else {
        let msg = "⚠️ reset already in progress, try again shortly";
        if chat != 0 {
            s.tg.send_msg(chat, thread_id, msg, None).await;
        }
        return Err(msg.into());
    };

    let pane = match split_reset_target(target) {
        Ok(th) => s
            .topics
            .storage
            .get_pane(th)
            .ok_or_else(|| format!("no pane found for topic #{th}"))?,
        Err(p) => p,
    };

    // Fail-closed like the paced reset: degraded reads must abort, never
    // mint tag-default titles for a pane whose facts are unknown.
    let (Some(spaces), Some(facts), Some(tabs)) = (
        list_workspaces(&s.cfg.socket).await.ok(),
        pane_facts(&s.cfg.socket).await.ok(),
        tab_labels(&s.cfg.socket).await.ok(),
    ) else {
        let msg = "⚠️ reset aborted: herdr read failed, topic untouched";
        if chat != 0 {
            s.tg.send_msg(chat, thread_id, msg, None).await;
        }
        return Err(msg.into());
    };
    let census = tab_census(&facts);

    let had_job = s.cancel_jobs_for(&pane).await;

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
        (
            "shell".to_string(),
            "ready".to_string(),
            title.map(|t| t.to_string()),
        )
    };

    let ws = facts.get(&pane).map(|f| f.ws.as_str()).unwrap_or("");
    let space = ws_label(&spaces, ws);

    // Same tab-core source as the watchdog: a verbatim the watchdog
    // keeps survives the single-topic migration too.
    let (tab, multi) = tab_of(&facts, &tabs, &census, &pane);
    let tag = s.topics.tag_for(&pane, &kind);
    let core = naming_core(tab, &tag, multi);
    let names = ResetNames {
        card: raw_title.as_deref(),
        core: core.as_deref(),
        multi,
    };

    let old_thread = s.topics.storage.get_thread(&pane);
    match s
        .topics
        .reset_topic(&pane, &kind, space, &status, names, None)
        .await
    {
        Some(new_th) => {
            let old_desc = old_thread
                .map(|t| format!("#{t}"))
                .unwrap_or_else(|| "none".into());
            let suffix = if had_job {
                " (running prompt cancelled — re-prompt to resume)"
            } else {
                ""
            };
            let msg = format!(
                "✅ Reset topic for {pane} ({kind}): old {old_desc} → new #{new_th}{suffix}"
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_reset_target_thread_vs_pane() {
        assert_eq!(split_reset_target("#627"), Ok(627));
        assert_eq!(split_reset_target("627"), Ok(627));
        assert_eq!(split_reset_target("  #627  "), Ok(627));
        assert_eq!(split_reset_target("w1:p2"), Err("w1:p2".to_string()));
        assert_eq!(split_reset_target(""), Err("".to_string()));
    }
}
