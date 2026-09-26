//! Single topic reset: deletes a single pane/topic on Telegram and recreates
//! it fresh (preserving queue messages and updating mappings).
use crate::{
    handlers::title_rules::{tab_census, tab_of, title_core_for},
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

/// Step-3 dead-topic prune: split from `reset` (300-line file limit).
/// Deletes topics for panes gone from Herdr. Snapshot-id delete plus a
/// generation gate before the retire: `mappings` predates minutes of
/// paced sleeps, so a live foreign mapping now is a remint whose fresh
/// jobs/state must survive (kill/reconcile_close parity).
pub(crate) async fn prune_dead_topics(
    s: &AppState,
    mappings: std::collections::HashMap<String, i64>,
    live_panes: &std::collections::HashSet<String>,
) -> usize {
    let mut dead_deleted = 0;
    for (pane, thread) in mappings {
        if !live_panes.contains(&pane) {
            println!("[reset] deleting dead topic #{thread} for {pane}");
            if s.topics.delete_topic_for_thread(&pane, thread).await {
                dead_deleted += 1;
            }
            let cur = s.topics.all_mappings().get(&pane).copied();
            if cur != Some(thread) && cur.is_some() {
                tokio::time::sleep(super::reset::RESET_STEP_DELAY).await;
                continue;
            }
            s.cancel_jobs_for(&pane).await;
            s.clear_pane(&pane).await;
            tokio::time::sleep(super::reset::RESET_STEP_DELAY).await;
        }
    }
    dead_deleted
}

/// Spawned single-topic reset: ~6 Telegram RPCs + herdr reads must not
/// stall the sequential pump behind the requesting surface. Owned
/// target for 'static. Single source for the forum/General/DM arms.
pub fn spawn_single_topic_reset(s: &AppState, chat: i64, thread_id: Option<i64>, target: String) {
    let s2 = s.clone();
    tokio::spawn(async move {
        let _ = run_single_topic_reset(&s2, chat, thread_id, &target).await;
    });
}

/// Never-silent Err arm (single source, structural-test pinned): the
/// spawn discards Err — a bare `return Err` looks wedged with no ack.
/// Sends only when `chat != 0` (chat 0 = internal/structural call — no
/// surface, no network), always returns Err(msg).
async fn refuse_reset(s: &AppState, chat: i64, thread_id: Option<i64>, msg: &str) -> Res<String> {
    if chat != 0 {
        s.tg.send_msg(chat, thread_id, msg, None).await;
    }
    Err(msg.to_string().into())
}

pub async fn run_single_topic_reset(
    s: &AppState,
    chat: i64,
    thread_id: Option<i64>,
    target: &str,
) -> Res<String> {
    let Some(forum) = s.cfg.forum else {
        return refuse_reset(s, chat, thread_id, crate::ui::RESET_FORUM_ONLY).await;
    };

    // F9: permission guard
    if let Ok(perms) = s.tg.check_forum_permissions(forum).await
        && !perms.can_manage_topics
    {
        return refuse_reset(s, chat, thread_id, crate::ui::RESET_NO_PERM).await;
    }

    // Share the paced-reset lock: without it the watchdog renames the
    // topic mid-mint and a concurrent paced reset double-migrates it.
    let Some(_guard) = super::reset::try_begin_reset() else {
        return refuse_reset(s, chat, thread_id, crate::ui::RESET_BUSY).await;
    };

    let pane = match split_reset_target(target) {
        Ok(th) => match s.topics.storage.get_pane(th) {
            Some(p) => p,
            // Never silent (spawn discards Err — silence looks wedged).
            None => {
                let msg = format!("no pane found for topic #{th}");
                return refuse_reset(s, chat, thread_id, &msg).await;
            }
        },
        Err(p) => p,
    };

    // Fail-closed like the paced reset: degraded reads must abort, never
    // mint tag-default titles for a pane whose facts are unknown.
    let (Some(spaces), Some(facts), Some(tabs)) = (
        list_workspaces(&s.cfg.socket).await.ok(),
        pane_facts(&s.cfg.socket).await.ok(),
        tab_labels(&s.cfg.socket).await.ok(),
    ) else {
        return refuse_reset(
            s,
            chat,
            thread_id,
            "⚠️ reset aborted: herdr read failed, topic untouched",
        )
        .await;
    };
    let census = tab_census(&facts);

    // Fail-closed like the reads above: a mistyped pane must refuse,
    // never mint a ghost topic for a name herdr never reported.
    if !facts.contains_key(&pane) {
        let msg = format!("⚠️ no such pane `{pane}` — see /agents");
        return refuse_reset(s, chat, thread_id, &msg).await;
    }

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
    // keeps survives the single-topic migration too (labeled splits
    // use the pane label — `title_core_for` parity, no flap).
    let (tab, multi) = tab_of(&facts, &tabs, &census, &pane);
    let tag = s.topics.tag_for(&pane, &kind);
    let pane_label = facts.get(&pane).and_then(|f| f.label.as_deref());
    let core = title_core_for(tab, &tag, multi, pane_label);
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
            super::dialog::retire_dialog(s, &pane).await;
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
            refuse_reset(s, chat, thread_id, &msg).await
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

    #[tokio::test]
    async fn test_refuse_reset_chat0_still_errs_silently() {
        // chat 0 = no surface (never network); the Err still carries
        // the message so the spawn's discard never eats the verdict.
        let (s, _dir) = crate::state::cancel::isolated_state();
        let e = refuse_reset(&s, 0, None, "boom").await.unwrap_err();
        assert_eq!(e.to_string(), "boom");
    }

    #[test]
    fn test_run_single_topic_reset_never_bare_err() {
        // Structural pin: every Err arm routes through refuse_reset
        // (never silent when a chat surface exists). A bare
        // `return Err(` would ship a wedged, unacked reset. Slice to
        // the fn body — this test's own string literal must not self-match.
        let src = include_str!("reset_single.rs");
        let body = &src[..src.find("#[cfg(test)]").expect("tests module")];
        assert!(
            !body.contains("return Err("),
            "reset Err arms must go through refuse_reset"
        );
    }
}
