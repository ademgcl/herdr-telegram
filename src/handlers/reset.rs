//! Paced reset of forum topics:
//! Deletes existing topics one by one with rate-limit pacing (~1.5s delay),
//! cleans local topic mappings, and re-syncs fresh topics from Herdr.
//!
//! AUDIT CONSTRAINT (D1):
//! Herdr access during reset is strictly READ-ONLY (`list_agents`,
//! `list_workspaces`, `pane_facts`, `list_panes`). Never calls `spawn`,
//! `pane.close`, `rename_pane`, or `send_keys`/`send_input`.

use crate::{
    herdr::{
        client::{list_agents, list_panes, list_workspaces},
        labels::pane_facts,
    },
    state::AppState,
    ui::ws_label,
};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const RESET_STEP_DELAY: Duration = Duration::from_millis(1500);

static RESET_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct ResetGuard;

impl Drop for ResetGuard {
    fn drop(&mut self) {
        RESET_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

pub async fn run_paced_reset(s: &AppState, chat: i64, thread_id: Option<i64>) {
    if s.cfg.forum.is_none() {
        s.tg.send_msg(
            chat,
            thread_id,
            "⚠️ Reset is only available in forum supergroup mode",
            None,
        )
        .await;
        return;
    }

    if RESET_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        s.tg.send_msg(
            chat,
            thread_id,
            "⚠️ Paced reset is already in progress. Please wait for it to finish.",
            None,
        )
        .await;
        return;
    }
    let _guard = ResetGuard;

    let mappings = s.topics.all_mappings();
    let to_delete_count = mappings.len();
    s.tg.send_msg(
        chat,
        thread_id,
        &format!(
            "🔄 Starting paced reset: deleting {to_delete_count} topic(s) and resyncing from Herdr (read-only)…"
        ),
        None,
    )
    .await;

    // Step 1: Paced deletion (~1.5s delay, 429 retry-safe)
    let mut deleted = 0;
    let mut failed = Vec::new();
    for (pane, thread) in mappings {
        println!("[reset] deleting topic #{thread} for {pane}");
        if s.topics.delete_topic(&pane).await {
            deleted += 1;
        } else {
            failed.push((pane, thread));
        }
        tokio::time::sleep(RESET_STEP_DELAY).await;
    }

    // Step 2: Clear stored titles and tags. Topics whose deletion
    // failed still exist on Telegram: keep their mappings so step 4
    // reuses them instead of minting duplicates.
    s.topics.clear_all();
    for (pane, thread) in &failed {
        s.topics.restore_mapping(pane.clone(), *thread);
    }

    // Step 3: Pure read from Herdr (AUDIT: zero mutation on Herdr)
    let agents = list_agents(&s.cfg.socket).await.unwrap_or_default();
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let facts = pane_facts(&s.cfg.socket).await.unwrap_or_default();
    let mut live_panes = HashSet::new();
    let mut created = 0;

    // Step 4: Re-sync topics paced (~1.5s delay)
    for r in &agents {
        live_panes.insert(r.pane.clone());
        let space = ws_label(&spaces, &r.ws);
        if s.topics.sync_topic(&r.pane, &r.kind, space).await.is_some() {
            created += 1;
            if let Some(f) = facts.get(&r.pane)
                && let Some(label) = f.label.as_deref().filter(|l| !l.trim().is_empty())
            {
                s.topics.sync_title(&r.pane, label).await;
            }
        }
        tokio::time::sleep(RESET_STEP_DELAY).await;
    }

    // Also re-sync any agentless live shell panes
    if let Ok(panes) = list_panes(&s.cfg.socket).await {
        for pane in panes {
            if !live_panes.contains(&pane) {
                let ws = facts.get(&pane).map(|f| f.ws.as_str()).unwrap_or("");
                let space = ws_label(&spaces, ws);
                if s.topics.sync_topic(&pane, "shell", space).await.is_some() {
                    created += 1;
                    if let Some(f) = facts.get(&pane)
                        && let Some(label) = f.label.as_deref().filter(|l| !l.trim().is_empty())
                    {
                        s.topics.sync_title(&pane, label).await;
                    }
                }
                tokio::time::sleep(RESET_STEP_DELAY).await;
            }
        }
    }

    let summary = if failed.is_empty() {
        format!(
            "✅ Paced reset complete: deleted {deleted} topic(s), recreated {created} topic(s)."
        )
    } else {
        format!(
            "✅ Paced reset complete: deleted {deleted} topic(s), recreated {created} topic(s), {} failed (kept, retried by next /reset).",
            failed.len()
        )
    };
    s.tg.send_msg(chat, thread_id, &summary, None).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_delay_constant() {
        assert_eq!(RESET_STEP_DELAY, Duration::from_millis(1500));
    }
}
