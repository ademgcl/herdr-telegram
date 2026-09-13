use std::collections::HashSet;
use crate::{
    herdr::client::list_agents, notifier::status::observe_status, state::AppState,
};

pub async fn reconcile(s: &AppState, silent: bool, src: &str) {
    let Ok(rows) = list_agents(&s.cfg.socket).await else { return };

    let mut live_panes = HashSet::new();

    for r in &rows {
        live_panes.insert(r.pane.clone());
        // Silent or not, this ensures the topic + pin exist; non-silent
        // also posts cards for genuine transitions.
        observe_status(s, &r.pane, &r.status, silent, src).await;
    }

    // Clean up or close topics for agents that are no longer live
    if s.cfg.forum.is_some() && !silent {
        let stored = s.topics.all_mappings();
        for (pane, _) in stored {
            if !live_panes.contains(&pane) {
                s.topics.close_topic(&pane).await;
                s.topics.remove_mapping(&pane);
            }
        }
    }
}
