use std::collections::HashSet;
use crate::{
    herdr::client::{list_agents, list_panes},
    notifier::status::observe_status,
    state::AppState,
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

    // Stored panes with no agent are shells (quit) or dead (closed).
    // Shells keep their topic with the shell badge and zero alerts;
    // only truly gone panes get closed.
    if s.cfg.forum.is_some() {
        let stored = s.topics.all_mappings();
        let missing: Vec<String> = stored
            .keys()
            .filter(|p| !live_panes.contains(*p))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let panes = list_panes(&s.cfg.socket).await.unwrap_or_default();
            for pane in missing {
                if panes.contains(&pane) {
                    s.status.lock().await.insert(pane.clone(), "shell".to_string());
                    s.topics.mark_shell(&pane).await;
                } else if !silent {
                    s.topics.close_topic(&pane).await;
                    s.topics.remove_mapping(&pane);
                }
            }
        }
    }
}
