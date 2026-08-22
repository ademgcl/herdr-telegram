use std::collections::HashSet;
use crate::{
    herdr::client::{list_agents, list_workspaces},
    notifier::status::observe_status,
    state::AppState,
};

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Ok(rows) = list_agents(&s.cfg.socket).await else { return };
    let spaces = list_workspaces(&s.cfg.socket).await.unwrap_or_default();

    let mut live_panes = HashSet::new();

    for r in &rows {
        live_panes.insert(r.pane.clone());
        observe_status(s, &r.pane, &r.status, silent, src).await;

        // Ensure topic exists in forum group if enabled
        if s.cfg.forum.is_some() {
            let space_label = spaces
                .iter()
                .find(|w| w.id == r.ws)
                .map(|w| w.label.as_str())
                .unwrap_or(&r.ws);
            s.topics.ensure_topic(&r.pane, &r.kind, space_label, &r.status).await;
        }
    }

    // Clean up or close topics for agents that are no longer live
    if s.cfg.forum.is_some() && !silent {
        let stored = s.topics.all_mappings();
        for (pane, _) in stored {
            if !live_panes.contains(&pane) {
                s.topics.close_topic(&pane, "agent", "done").await;
                s.topics.remove_mapping(&pane);
            }
        }
    }
}
