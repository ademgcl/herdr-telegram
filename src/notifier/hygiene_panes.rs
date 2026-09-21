//! Age-based waiter/notice pruning (split from `hygiene`: 300-line
//! file limit) + cached single `list_panes` per tick (shared with the
//! forum block: 1 RPC, not 2). Fail-open: Err keeps everything.
use crate::{
    herdr::client::list_panes,
    state::{
        AppState,
        guard::{
            DEBOUNCE_STALE_SECS, KEYWAIT_STALE_SECS, RUNWAIT_STALE_SECS, TYPEWAIT_STALE_SECS,
            claim_stale,
        },
    },
};
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

/// Age-prune input waiters, notice stamps, debounce arms, and done
/// stamps (no RPC, never pane-liveness): stale arms firing later would
/// execute dead input as commands, and access-only pruning grows
/// unbounded across chats (new map needs expiry + prune).
pub(crate) async fn prune_age(s: &AppState) {
    let now = std::time::Instant::now();
    s.runwait
        .lock()
        .await
        .retain(|_, (_, at)| !claim_stale(*at, now, RUNWAIT_STALE_SECS));
    s.keywait
        .lock()
        .await
        .retain(|_, (_, at)| !claim_stale(*at, now, KEYWAIT_STALE_SECS));
    s.typewait
        .lock()
        .await
        .retain(|_, (_, at)| !claim_stale(*at, now, TYPEWAIT_STALE_SECS));
    s.nagged
        .lock()
        .await
        .retain(|_, at| !claim_stale(*at, now, crate::types::NAGGED_SECS));
    s.stale_nagged
        .lock()
        .await
        .retain(|_, at| !claim_stale(*at, now, crate::types::STALE_SECS));
    s.stale_tap_nagged
        .lock()
        .await
        .retain(|_, at| !claim_stale(*at, now, crate::types::STALE_SECS));
    // Debounce arms own a 15s task + bounded retries: older owns no live
    // task (or a wedged one). Done stamps live as long as the arms that
    // consult them: settle cards suppress on `last_done > armed_at` for
    // arms up to DEBOUNCE_STALE_SECS old — pruning on the shorter quiet
    // window forgot delivered finals while a stale arm still lived and
    // let one stale duplicate card through.
    s.debounce
        .lock()
        .await
        .retain(|_, (_, at)| !claim_stale(*at, now, DEBOUNCE_STALE_SECS));
    s.last_done
        .lock()
        .await
        .retain(|_, t| t.elapsed() < std::time::Duration::from_secs(DEBOUNCE_STALE_SECS));
}
