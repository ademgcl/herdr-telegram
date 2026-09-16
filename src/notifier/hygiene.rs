//! Dead-pane hygiene: mode-independent reaping of jobs, durable intent,
//! and per-pane maps for externally-closed panes. Split from `reconcile`
//! (300-line file limit).
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
            eprintln!("[reconcile] pane list failed, keeping topics: {e}");
            None
        }
    }
}

/// Reap mode-independent orphans: DM-mode prompts can orphan jobs,
/// durable intent, and per-pane maps for externally-closed panes (no
/// topic mapping exists to trigger the forum close flow). Live panes
/// are skipped; truly gone ones are cancelled + cleared. Fail-open:
/// never wipe intents on a failed list call (Err) or a transient empty
/// Ok([]).
pub(crate) async fn reap_orphans(s: &AppState, pane_list: &mut Option<HashSet<String>>) {
    let mut known: Vec<String> = s.jobs.lock().await.keys().cloned().collect();
    known.extend(s.pending.lock().await.keys().cloned());
    // Armed input waiters also pin a pane: a keywait/typewait for an
    // externally-closed shell (no job, no intent, DM mode) must die
    // with it instead of eating the next message as dead input.
    known.extend(s.keywait.lock().await.values().cloned());
    known.extend(s.runwait.lock().await.values().cloned());
    known.extend(s.typewait.lock().await.values().cloned());
    if known.is_empty() {
        return;
    }
    match panes_once(s, pane_list).await {
        Some(live) if live.is_empty() => {
            eprintln!("[reconcile] pane list empty, keeping intents");
        }
        Some(live) => {
            // Retain live-only (not live∪known): known includes
            // the just-cleared dead panes, so ∪ would keep
            // everything clear_pane missed.
            for pane in &known {
                if !live.contains(pane) {
                    // Job-only retire: the durable intent is
                    // boot-recover's reporter for dead panes
                    // ("pane gone before reply arrived") — a loud
                    // retire would wipe it the same tick the
                    // forum branch above restored it, and a
                    // "✋ cancelled" card on a dead pane is noise.
                    // Waiters still die via clear_pane below, maps
                    // via the retains; lingering intent is bounded
                    // by recover's 24h stale drop.
                    s.cancel_job_only_for(pane).await;
                    s.clear_pane(pane).await;
                }
            }
            s.status.lock().await.retain(|p, _| live.contains(p));
            s.seen.lock().await.retain(|p, _| live.contains(p));
            s.last_done.lock().await.retain(|p, _| live.contains(p));
            // Atomic with status (order status→last_change).
            s.last_change.lock().await.retain(|p, _| live.contains(p));
            s.limit_alert.lock().await.retain(|p, _| live.contains(p));
            s.limit_seen.lock().await.retain(|p, _| live.contains(p));
            s.limit_miss.lock().await.retain(|p, _| live.contains(p));
            s.debounce.lock().await.retain(|p, _| live.contains(p));
            s.blocked_sig.lock().await.retain(|p, _| live.contains(p));
            s.modelop.lock().await.retain(|p| live.contains(p));
            s.blockop.lock().await.retain(|p| live.contains(p));
            // Typing tasks for dead panes: abort, don't leak.
            for (_, h) in s
                .typing_tasks
                .lock()
                .await
                .extract_if(|p, _| !live.contains(p))
                .collect::<Vec<_>>()
            {
                h.abort();
            }
            // Reply targets pointing at dead panes (order
            // torder→targets, as in remember).
            {
                let mut ord = s.torder.lock().await;
                let mut map = s.targets.lock().await;
                map.retain(|_, p| live.contains(p));
                let live_keys: std::collections::HashSet<(i64, i64)> =
                    map.keys().cloned().collect();
                ord.retain(|k| live_keys.contains(k));
            }
        }
        None => {}
    }
}
