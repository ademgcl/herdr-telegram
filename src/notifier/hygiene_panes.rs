//! Cached single `list_panes` per tick (split from `hygiene`:
//! 300-line file limit). Shared with the forum block: 1 RPC, not 2.
//! Fail-open: Err keeps everything.
use crate::{herdr::client::list_panes, state::AppState};
use std::collections::HashSet;

/// Cached single `list_panes` per tick (shared with the forum block):
/// 1 RPC, not 2. Fail-open: Err keeps everything.
pub(crate) async fn panes_once(
    s: &AppState,
    cache: &mut Option<HashSet<String>>,
) -> Option<HashSet<String>> {
    if let Some(p) = cache {
        return Some(p.clone());
    }
    match list_panes(&s.cfg.socket).await {
        Ok(l) => {
            let set: HashSet<String> = l.into_iter().collect();
            *cache = Some(set.clone());
            Some(set)
        }
        Err(e) => {
            eprintln!(
                "[reconcile] pane list failed, keeping topics: {}",
                crate::types::mask_home(&e.to_string())
            );
            None
        }
    }
}
