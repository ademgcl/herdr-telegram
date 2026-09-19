//! Paced reset of forum topics:
//! Deletes existing topics one by one with rate-limit pacing (~1.5s delay),
//! cleans local topic mappings, and re-syncs fresh topics from Herdr.
//!
//! AUDIT CONSTRAINT (D1):
//! Herdr access during reset is strictly READ-ONLY (`list_agents`,
//! `list_workspaces`, `pane_facts`, `list_panes`, `tab_labels`). Never calls `spawn`,
//! `pane.close`, `rename_pane`, or `send_keys`/`send_input`.

use crate::{
    handlers::title_rules::{tab_census, tab_of, title_core_for},
    herdr::{
        client::{list_agents, list_panes, list_workspaces},
        labels::{facts_contradict, pane_facts, tab_labels},
    },
    state::AppState,
    topics::manager::ResetNames,
    ui::ws_label,
};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const RESET_STEP_DELAY: Duration = Duration::from_millis(1500);

pub use super::reset_single::*;

static RESET_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// True while a paced reset runs: topic creators must not mint (only
/// reuse) until it ends, or in-flight mappings are wiped into orphans +
/// later doubles. Set before any snapshot; auto-cleared.
/// Watchdog/notifier also consult it: reconcile, spontaneous cards and
/// event-driven observations pause while it holds (no 429 storm), while
/// read-only memory updates continue.
pub fn is_resetting() -> bool {
    RESET_IN_PROGRESS.load(Ordering::SeqCst)
}

/// RAII reset lock: dropping it releases. See [`try_begin_reset`].
pub struct ResetGuard;

impl Drop for ResetGuard {
    fn drop(&mut self) {
        RESET_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Claim the reset lock (paced and single-topic resets share it, so a
/// single reset can neither run inside a paced reset nor let the
/// watchdog rename topics mid-mint). None = already held.
pub fn try_begin_reset() -> Option<ResetGuard> {
    RESET_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| ResetGuard)
}

pub async fn run_paced_reset(s: &AppState, chat: i64, thread_id: Option<i64>) {
    let forum = match s.cfg.forum {
        Some(f) => f,
        None => {
            s.tg.send_msg(chat, thread_id, crate::ui::RESET_FORUM_ONLY, None)
                .await;
            return;
        }
    };

    // F9: permission guard — ensure bot can manage topics before reset deletes/recreates
    match s.tg.check_forum_permissions(forum).await {
        Ok(perms) if !perms.can_manage_topics => {
            s.tg.send_msg(chat, thread_id, crate::ui::RESET_NO_PERM, None)
                .await;
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "[reset] permission check warning (proceeding): {}",
                s.tg.redact(&e.to_string())
            );
        }
    }

    let Some(_guard) = try_begin_reset() else {
        s.tg.send_msg(
            chat,
            thread_id,
            "⚠️ Paced reset is already in progress. Please wait for it to finish.",
            None,
        )
        .await;
        return;
    };

    // Step 0: read Herdr BEFORE any delete — an outage aborts with
    // mappings untouched, never wiping topics we cannot rebuild.
    // Fail-closed on EVERY read (not just agents): a degraded fetch
    // mints tag-default titles (facts/tabs) or deletes live shell
    // topics Step 3 then reads as dead (panes) or mis-spaces them.
    let agents = match list_agents(&s.cfg.socket).await {
        Ok(a) => a,
        Err(_) => {
            s.tg.send_msg(
                chat,
                thread_id,
                &format!(
                    "{} — reset aborted, mappings untouched",
                    crate::ui::HERDR_UNREACHABLE
                ),
                None,
            )
            .await;
            return;
        }
    };

    let (Some(spaces), Some(facts), Some(shell_panes), Some(tabs)) = (
        list_workspaces(&s.cfg.socket).await.ok(),
        pane_facts(&s.cfg.socket).await.ok(),
        list_panes(&s.cfg.socket).await.ok(),
        tab_labels(&s.cfg.socket).await.ok(),
    ) else {
        s.tg.send_msg(
            chat,
            thread_id,
            "⚠️ reset aborted: herdr read failed, mappings untouched",
            None,
        )
        .await;
        return;
    };
    // Tab-name source (read-only, like the other fetches): reset names
    // topics from the same tab core as the watchdog (`naming_core`), so
    // a verbatim the watchdog keeps survives the migration.
    let census = tab_census(&facts);

    let mappings = s.topics.all_mappings();
    // Hollow-read guard (mirrors the watchdog): every facts pane must
    // appear in agents or shells (all three list the same panes) — a
    // contradiction is a degraded read, and Step 3 would delete live
    // topics as dead. Genuine empties (fresh installs, all closed)
    // have no contradiction, so prune proceeds.
    if !mappings.is_empty() && facts_contradict(&agents, &shell_panes, &facts) {
        s.tg.send_msg(
            chat,
            thread_id,
            "⚠️ reset aborted: herdr reads came back empty, mappings untouched",
            None,
        )
        .await;
        return;
    };
    let to_reset_count = mappings.len();
    s.tg.send_msg(
        chat,
        thread_id,
        &format!(
            "🔄 Starting paced reset: migrating {to_reset_count} topic(s) and resyncing from Herdr (read-only)…"
        ),
        None,
    )
    .await;

    // Drain in-flight creates before the reset
    for _ in 0..400 {
        if s.topics.creating_len() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let mut live_panes = HashSet::new();
    let mut migrated = 0;
    let mut failed = Vec::new();

    // Step 1: Migrate agent topics (F6 copy recent msgs + F2 identity card)
    for r in &agents {
        live_panes.insert(r.pane.clone());
        let space = ws_label(&spaces, &r.ws);
        let raw_title = facts
            .get(&r.pane)
            .and_then(|f| f.label.as_deref())
            .filter(|l| !l.trim().is_empty())
            .or_else(|| {
                if r.title.trim().is_empty() {
                    None
                } else {
                    Some(r.title.as_str())
                }
            });
        s.cancel_jobs_for(&r.pane).await;
        let (tab, multi) = tab_of(&facts, &tabs, &census, &r.pane);
        let tag = s.topics.tag_for(&r.pane, &r.kind);
        let pane_label = facts.get(&r.pane).and_then(|f| f.label.as_deref());
        let core = title_core_for(tab, &tag, multi, pane_label);
        let names = ResetNames {
            card: raw_title,
            core: core.as_deref(),
            multi,
        };
        match s
            .topics
            .reset_topic(&r.pane, &r.kind, space, &r.status, names, None)
            .await
        {
            Some(_) => {
                migrated += 1;
                super::dialog::retire_dialog(s, &r.pane).await;
            }
            None => {
                failed.push(r.pane.clone());
            }
        }
        tokio::time::sleep(RESET_STEP_DELAY).await;
    }

    // Step 2: Migrate live agentless shell panes
    for pane in shell_panes {
        if !live_panes.contains(&pane) {
            live_panes.insert(pane.clone());
            let ws = facts.get(&pane).map(|f| f.ws.as_str()).unwrap_or("");
            let space = ws_label(&spaces, ws);
            let raw_title = facts
                .get(&pane)
                .and_then(|f| f.label.as_deref())
                .filter(|l| !l.trim().is_empty());
            s.cancel_jobs_for(&pane).await;
            let (tab, multi) = tab_of(&facts, &tabs, &census, &pane);
            let tag = s.topics.tag_for(&pane, "shell");
            let pane_label = facts.get(&pane).and_then(|f| f.label.as_deref());
            let core = title_core_for(tab, &tag, multi, pane_label);
            let names = ResetNames {
                card: raw_title,
                core: core.as_deref(),
                multi,
            };
            match s
                .topics
                .reset_topic(&pane, "shell", space, "ready", names, None)
                .await
            {
                Some(_) => {
                    migrated += 1;
                    super::dialog::retire_dialog(s, &pane).await;
                }
                None => {
                    failed.push(pane.clone());
                }
            }
            tokio::time::sleep(RESET_STEP_DELAY).await;
        }
    }

    // Step 3: dead-topic prune (split to `reset_single`: 300-line limit).
    let dead_deleted = super::reset_single::prune_dead_topics(s, mappings, &live_panes).await;

    let mut summary = format!(
        "✅ Paced reset complete: migrated {migrated} topic(s) (queue carried over), cleaned {dead_deleted} dead topic(s). Any running prompt was cancelled — re-prompt if it went quiet."
    );
    if !failed.is_empty() {
        summary.push_str(&format!(
            " {} failed (retried by next /reset).",
            failed.len()
        ));
    }
    s.tg.send_msg(chat, thread_id, &summary, None).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_delay_constant() {
        assert_eq!(RESET_STEP_DELAY, Duration::from_millis(1500));
    }

    #[test]
    fn test_is_resetting_flag() {
        assert!(!is_resetting());
    }
}
