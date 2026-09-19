//! Reconcile tail: DM flip + hygiene + titles. Split from `reconcile`
//! (300-line file limit).
use crate::{
    handlers::titles::sync_titles_with,
    herdr::client::list_workspaces,
    herdr::labels::{pane_facts, tab_labels},
    notifier::hygiene::reap_orphans,
    notifier::hygiene_flip::flip_dm_shells,
    state::AppState,
    types::AgentRow,
};
use std::collections::HashSet;

/// DM flip + dead-pane hygiene + 1:1 titles (tail of `reconcile`).
pub async fn reconcile_tail(
    s: &AppState,
    rows: &[AgentRow],
    live_panes: &HashSet<String>,
    pane_list: &mut Option<HashSet<String>>,
) {
    // DM mode has no topics, but `status` still drives the limit
    // scanner — flip shell-reused panes (status + episode, no report).
    if s.cfg.forum.is_none() {
        flip_dm_shells(s, live_panes, pane_list).await;
    }

    // Mode-independent dead-pane hygiene (jobs, intent, per-pane maps
    // for externally-closed panes). Fail-open on Err/empty (see hygiene).
    reap_orphans(s, pane_list).await;

    // 1:1 tab↔topic titles (herdr tab names win; native TG renames
    // flow back via forum_topic_edited). Reuses this tick's rows plus
    // one spaces/facts/tabs fetch — no extra list_agents per tick.
    // Fail-closed: any degraded fetch skips the tick (never tag/? mass
    // reformats); an Ok-but-empty facts map is still safe (unknown panes
    // skip per-pane inside).
    if s.cfg.forum.is_some() {
        let (Some(spaces), Some(facts), Some(tabs)) = (
            list_workspaces(&s.cfg.socket).await.ok(),
            pane_facts(&s.cfg.socket).await.ok(),
            tab_labels(&s.cfg.socket).await.ok(),
        ) else {
            return;
        };
        sync_titles_with(s, rows, &spaces, &facts, &tabs).await;
    }
}
