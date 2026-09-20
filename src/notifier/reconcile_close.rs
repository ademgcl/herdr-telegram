//! Dead-pane silent close: split from `reconcile` (300-line file limit).
//! Compare-and-delete throughout: a remint racing the tick keeps its
//! fresh topic, mapping, and work — the retire only runs for the corpse.
use crate::state::AppState;

/// Silent close for one confirmed-dead pane. Returns true when the caller
/// should `continue` (reminted mid-RPC — retire skipped to protect fresh
/// work), false when the retire below should run.
pub(crate) async fn close_dead_pane(s: &AppState, pane: &str) -> bool {
    let thread = s.topics.all_mappings().get(pane).copied();
    // Snapshot the owed intent: the silent close below retires it, but
    // boot-recover's gone-notice is the designed reporter for dead panes
    // — restore it so the reply still arrives next boot (bounded by
    // recover's 24h stale drop). The flap self-terminates: a successful
    // close drops the mapping, so this runs at most once more.
    let owed = s.pending.lock().await.get(pane).cloned();
    // (close_topic_for_thread compare-deletes on success. Snapshot None
    // means no known thread: skip the RPC entirely — any mapping present
    // now is by definition a remint whose fresh topic must survive.)
    // NOTE: no pre-RPC re-read compare here by design — two back-to-back
    // reads with no await between cannot differ (no yield point), so that
    // check is dead code; the real remint guards are the post-RPC CAS
    // below and close_topic_for_thread's own compare-delete.
    if let Some(t) = thread {
        s.topics.close_topic_for_thread(pane, t).await;
    }
    // Remint mid-RPC keeps its mapping (CAS fails): skip the retire — it
    // would kill fresh work and resurrect the corpse intent over it.
    // Re-check immediately before the retire: a remint landing in the
    // awaits above must not lose its fresh state to clear_pane below.
    if matches!(
        (thread, s.topics.all_mappings().get(pane).copied()),
        (_, Some(now)) if Some(now) != thread
    ) {
        return true;
    }
    s.cancel_jobs_for_quiet(pane).await;
    // Final generation gate: a remint slipping in during the cancel
    // awaits above keeps its waiters/guards/debounce — clear only the
    // corpse's (a live foreign mapping bails; our absence proceeds).
    let cur = s.topics.all_mappings().get(pane).copied();
    if cur != thread && cur.is_some() {
        return true;
    }
    s.clear_pane(pane).await;
    // Restore only when nothing newer owns the slot: a submit racing the
    // close RPCs above must win over the corpse's text (overwrite = lost
    // reply). Atomic check-and-set, ORIGINAL timestamp (a fresh stamp per
    // restore would defeat recover's 24h stale drop — vanished parity).
    if let Some(pp) = owed {
        s.remember_pending_cas_with_time(
            pane,
            (pp.chat, pp.thread, &pp.prompt),
            (pp.chat, pp.thread, &pp.prompt),
            Some(pp.started_unix),
        )
        .await;
    }
    false
}
