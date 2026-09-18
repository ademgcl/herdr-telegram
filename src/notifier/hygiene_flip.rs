//! DM-mode shell flips: split from `hygiene` (300-line file limit).
use super::hygiene::panes_once;
use crate::state::AppState;
use std::collections::HashSet;

/// DM-mode shell flip: no topics exist, but `status` still drives the
/// limit scanner — a PC-side quit would keep its last agent status
/// forever and quota words in ordinary shell output would buzz false
/// ❗ cards. Flip shell-reused panes (status + episode only — no topic,
/// no report); dead panes stay for `reap_orphans`.
pub(crate) async fn flip_dm_shells(
    s: &AppState,
    live_panes: &HashSet<String>,
    pane_list: &mut Option<HashSet<String>>,
) {
    let missing: Vec<String> = {
        let st = s.status.lock().await;
        st.keys()
            .filter(|p| !live_panes.contains(*p) && st.get(*p).map(|v| v != "shell").unwrap_or(false))
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let Some(panes) = panes_once(s, pane_list).await else {
        return;
    };
    if panes.is_empty() {
        return;
    }
    for pane in missing {
        if panes.contains(&pane) {
            s.status.lock().await.insert(pane.clone(), "shell".to_string());
            s.clear_limit_episode(&pane).await;
        }
    }
}
