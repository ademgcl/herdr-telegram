//! Dead-pane silent close: split from `reconcile` (300-line file limit).
//! Compare-and-delete throughout: a remint racing the tick keeps its
//! fresh topic, mapping, and work — the retire only runs for the corpse.
use crate::jobs::persist::PendingPrompt;
use crate::state::AppState;

/// Owed-slot race gate (single source for both close gates): true when
/// a submit owns the slot now — a stamp mismatch (or a fresh intent
/// where none was owed) means the racer's work must survive; skip the
/// retire.
pub(crate) async fn racer_owns_owed(
    s: &AppState,
    pane: &str,
    owed: Option<&PendingPrompt>,
) -> bool {
    match owed {
        Some(pp) => {
            !s.pending_matches_stamp(pane, pp.chat, pp.thread, &pp.prompt, pp.started_unix)
                .await
        }
        None => s.pending.lock().await.contains_key(pane),
    }
}

/// True when any work still owns the pane after quiet's LWW retire —
/// jobs OR pending (a pending-only shell successor has NO job, so a
/// jobs-only check wipes it). Bail before clear_pane would erase its
/// waiters/focus/status.
pub(crate) async fn successor_owns_slot(s: &AppState, pane: &str) -> bool {
    s.jobs.lock().await.contains_key(pane) || s.pending.lock().await.contains_key(pane)
}

/// Final generation gate (pure, tested): true only when a NEW mapping
/// appeared (a remint during the cancel awaits). `None` after our own
/// successful close is expected — clearing the corpse must still run.
/// Mirrors kill.rs's `cur != thread && cur.is_some()`.
pub(crate) fn generation_bails(cur: Option<i64>, thread: Option<i64>) -> bool {
    cur.is_some() && cur != thread
}

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
        // Gate the retire on the close verdict: a transient failure
        // keeps the mapping (callee compare-deletes only on success) —
        // retiring anyway would orphan the still-open topic with no
        // tracker, so the close is never retried. `topic_gone` counts
        // as success above, so corpse-prune still converges.
        if !s.topics.close_topic_for_thread(pane, t).await {
            return true;
        }
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
    // Pending race gate (before the strip + the blind clear below): a
    // submit landing during the close RPC above owns the pane now —
    // cancelling would wipe its fresh intent and the CAS restore would
    // resurrect the corpse over the vacancy.
    if racer_owns_owed(s, pane, owed.as_ref()).await {
        return true;
    }
    // Strip dead ⛔ buttons (clear_pane drops tracking without
    // stripping); contention-silent via shared helper.
    crate::handlers::dialog::resolve_cards_unless_held(s, pane).await;
    // Re-gate after the strip await: the first gate predates this RPC —
    // a submit landing in between owns the slot (quiet's fresh snapshot
    // would otherwise EAT it as the corpse's intent).
    if racer_owns_owed(s, pane, owed.as_ref()).await {
        return true;
    }
    // Quiet retire is LWW; afterwards ANY successor holding jobs OR
    // pending (pending-only shell intent included — no job yet) owns the
    // slot: bail before clear_pane wipes its waiters/focus/status and
    // before the corpse restore resurrects over live work.
    s.cancel_jobs_for_quiet(pane).await;
    if successor_owns_slot(s, pane).await {
        return true;
    }
    // Final generation gate: a remint slipping in during the cancel
    // awaits above keeps its waiters/guards/debounce — clear only the
    // corpse's (a NEW mapping appearing bails; `None` after our
    // successful close is the expected state, so the clear still runs —
    // kill.rs `cur != thread && cur.is_some()` parity).
    let cur = s.topics.all_mappings().get(pane).copied();
    if generation_bails(cur, thread) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::cancel::isolated_state;

    fn pp(prompt: &str, stamp: u64) -> PendingPrompt {
        PendingPrompt {
            chat: 1,
            thread: None,
            prompt: prompt.into(),
            started_unix: stamp,
        }
    }

    #[tokio::test]
    async fn test_racer_gate_stamp_pinned() {
        let (s, _dir) = isolated_state();
        // Owed still holding its stamp → not a racer; anything else is.
        s.pending.lock().await.insert("t:p1".into(), pp("hi", 42));
        assert!(!racer_owns_owed(&s, "t:p1", Some(&pp("hi", 42))).await);
        assert!(racer_owns_owed(&s, "t:p1", Some(&pp("hi", 99))).await);
        assert!(racer_owns_owed(&s, "t:p1", Some(&pp("other", 42))).await);
        s.pending.lock().await.remove("t:p1");
        assert!(racer_owns_owed(&s, "t:p1", Some(&pp("hi", 42))).await);
        // No owed intent: vacant is clean, a fresh submit owns it.
        assert!(!racer_owns_owed(&s, "t:p1", None).await);
        s.pending.lock().await.insert("t:p1".into(), pp("fresh", 1));
        assert!(racer_owns_owed(&s, "t:p1", None).await);
    }

    #[tokio::test]
    async fn test_successor_owns_slot_counts_pending_only() {
        // The judge's hole: quiet's LWW false + jobs map empty used to
        // fall through to clear_pane, wiping a pending-only successor's
        // waiters/focus/status. Pending counts as ownership too.
        let (s, _dir) = isolated_state();
        assert!(!successor_owns_slot(&s, "t:p1").await);
        s.pending.lock().await.insert("t:p1".into(), pp("live", 1));
        assert!(
            successor_owns_slot(&s, "t:p1").await,
            "pending-only successor must bail the close"
        );
        s.pending.lock().await.remove("t:p1");
        let job = crate::jobs::job::Job::new(vec![], 1, None);
        s.jobs.lock().await.insert("t:p1".into(), job);
        assert!(successor_owns_slot(&s, "t:p1").await);
    }

    #[test]
    fn test_generation_bails_only_on_new_mapping() {
        // Post-close `None` (our successful close wiped the mapping):
        // not a remint — clear/restore must still run. Before the fix
        // `cur != thread` was true here and the retire never ran for
        // any pane that ever had a topic.
        assert!(!generation_bails(None, Some(5)));
        // No mapping from the start, none appeared: clean corpse.
        assert!(!generation_bails(None, None));
        // Same mapping still there (close skipped / topic_gone path).
        assert!(!generation_bails(Some(5), Some(5)));
        // Remint during the cancel awaits: bail, keep fresh state.
        assert!(generation_bails(Some(6), Some(5)));
        // Mapping appeared where none was snapshotted.
        assert!(generation_bails(Some(5), None));
    }

    #[tokio::test]
    async fn test_close_dead_pane_corpse_clears_and_restores_original_stamp() {
        // Corpse path (no mapping, no RPC): quiet clears, clear_pane
        // drops corpse routing state, owed intent restores with its
        // ORIGINAL stamp for boot-recover (fresh stamp would defeat the
        // 24h stale drop).
        let (s, _dir) = isolated_state();
        s.pending.lock().await.insert("t:p1".into(), pp("hi", 42));
        s.status
            .lock()
            .await
            .insert("t:p1".into(), "working".into());
        s.set_focus("t:p1").await;
        s.debounce
            .lock()
            .await
            .insert("t:p1".into(), ("idle".into(), std::time::Instant::now()));
        let bailed = close_dead_pane(&s, "t:p1").await;
        assert!(!bailed, "clean corpse must run the retire");
        let restored = s.pending.lock().await.get("t:p1").cloned();
        let restored = restored.expect("owed intent restored for recover");
        assert_eq!(restored.prompt, "hi");
        assert_eq!(restored.started_unix, 42, "original stamp, never now");
        assert!(s.get_focus().await.is_none(), "corpse focus cleared");
        assert!(
            !s.status.lock().await.contains_key("t:p1"),
            "corpse status cleared"
        );
        assert!(!s.debounce.lock().await.contains_key("t:p1"));
    }
}
