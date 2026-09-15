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
    topics::names,
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

    // Step 0: read Herdr BEFORE any delete — an outage aborts with
    // mappings untouched, never wiping topics we cannot rebuild.
    let agents = match list_agents(&s.cfg.socket).await {
        Ok(a) => a,
        Err(_) => {
            s.tg.send_msg(
                chat,
                thread_id,
                "⚠️ reset aborted: herdr unreachable, mappings untouched",
                None,
            )
            .await;
            return;
        }
    };

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
    let mut deleted_panes = Vec::new();
    let mut failed = Vec::new();
    for (pane, thread) in mappings {
        println!("[reset] deleting topic #{thread} for {pane}");
        if s.topics.delete_topic(&pane).await {
            deleted += 1;
            deleted_panes.push(pane);
        } else {
            failed.push((pane, thread));
        }
        tokio::time::sleep(RESET_STEP_DELAY).await;
    }

    // Step 2: Clear stored titles and tags. Topics whose deletion
    // failed still exist on Telegram: their full identity (tag, title,
    // icon) survives so the re-sync reuses them instead of minting
    // renamed duplicates or clobbering user-custom icons.
    let mut kept = Vec::new();
    for (pane, thread) in &failed {
        kept.push((pane.clone(), *thread, s.topics.snapshot_identity(pane)));
    }
    s.topics.clear_all();
    for (pane, thread, (tag, title, icon)) in kept {
        s.topics.restore_identity(pane, thread, tag, title, icon);
    }

    // Retire jobs for deleted topics: watchers and typing loops point
    // at dead threads (delivery would fail-retry forever, typing would
    // spam General). Survivors keep theirs.
    for pane in &deleted_panes {
        s.cancel_jobs_for(pane).await;
        s.clear_pane(pane).await;
    }

    // Step 3: Pure read from Herdr (AUDIT: zero mutation on Herdr).
    // Agents were read up front (step 0 aborts on outage); spaces and
    // facts only shape labels, so they stay fail-open.
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();
    let facts = pane_facts(&s.cfg.socket).await.unwrap_or_default();
    let failed_set: HashSet<String> = failed.iter().map(|(p, _)| p.clone()).collect();
    let mut live_panes = HashSet::new();
    let mut created = 0;
    let mut reused = 0;

    // Step 4: Re-sync topics paced (~1.5s delay)
    for r in &agents {
        live_panes.insert(r.pane.clone());
        let space = ws_label(&spaces, &r.ws);
        if s.topics.sync_topic(&r.pane, &r.kind, space).await.is_some() {
            if failed_set.contains(&r.pane) {
                reused += 1;
            } else {
                created += 1;
            }
            if let Some(f) = facts.get(&r.pane)
                && let Some(label) = f.label.as_deref().filter(|l| !l.trim().is_empty())
            {
                let formatted = names::format_title(space, label, &r.kind);
                s.topics.sync_title(&r.pane, &formatted).await;
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
                    if failed_set.contains(&pane) {
                        reused += 1;
                    } else {
                        created += 1;
                    }
                    if let Some(f) = facts.get(&pane)
                        && let Some(label) = f.label.as_deref().filter(|l| !l.trim().is_empty())
                    {
                        let formatted = names::format_title(space, label, "shell");
                        s.topics.sync_title(&pane, &formatted).await;
                    }
                }
                tokio::time::sleep(RESET_STEP_DELAY).await;
            }
        }
    }

    let mut summary = format!(
        "✅ Paced reset complete: deleted {deleted} topic(s), recreated {created} topic(s)."
    );
    if !failed.is_empty() {
        summary.push_str(&format!(
            " {} failed (kept, retried by next /reset).",
            failed.len()
        ));
    }
    if reused > 0 {
        summary.push_str(&format!(" {reused} reused."));
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
}
