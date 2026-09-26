//! Transient retire paths: delete the pane's shared slot only when
//! `/transient` auto-remove is on — when off the message stays as
//! readable history AND as the next turn's canvas (still zero new
//! notification entries: reuse is always an edit). Deletes are gated on
//! the entry (mid, generation): a successor turn claiming the slot
//! (bump in `ensure_instant`) or reposting (new mid) owns it now and
//! the stale retire stands down. Split from `progress` (300-line file
//! limit) — call sites keep `progress::…` (re-exported there).
//! Fail-closed like `progress`: best-effort, never a lock across RPC.
use crate::state::{AppState, live::LiveSlot};

/// Map-ownership check (pure lock read, never across RPC): a detached
/// watcher must not post into its successor's turn.
pub(crate) async fn is_owner(
    s: &AppState,
    pane: &str,
    job: &std::sync::Arc<crate::jobs::job::Job>,
) -> bool {
    s.jobs
        .lock()
        .await
        .get(pane)
        .is_some_and(|j| std::sync::Arc::ptr_eq(j, job))
}

/// Delete one taken slot (auto-remove is on — the caller checked).
/// Unsent banked slots take without any RPC.
async fn delete_live(s: &AppState, pane: &str, slot: LiveSlot) {
    if slot.mid < 0 {
        return;
    }
    println!(
        "[live] retire {pane} m{}: auto-remove on, deleting",
        slot.mid
    );
    s.tg.delete_msg(slot.chat, slot.mid).await;
    s.forget_target(slot.chat, slot.mid).await;
}

/// Generation-gated retire (finalize parity): only the turn holding the
/// entry (mid, generation) deletes — a successor that claimed the slot
/// (bumped generation, same message) or reposted (new mid) owns it now
/// and the stale retire stands down. When auto-remove is off this is a
/// no-op: the slot survives for the next turn to reuse.
pub async fn clear_live_if_epoch(s: &AppState, pane: &str, live_entry: Option<(i64, u64)>) {
    if !s.transient_remove() {
        return;
    }
    let Some((mid0, turn0)) = live_entry else {
        return;
    };
    let Some(sl) = s.live_get(pane).await else {
        return;
    };
    if (sl.mid, sl.turn) != (mid0, turn0) {
        println!("[live] retire {pane}: slot moved on, keeping");
        return;
    }
    // Only the take winner deletes: a successor claiming the slot
    // between the snapshot and here loses our take, and its canvas
    // must survive our stale retire.
    let Some(taken) = s.live_take_if(pane, sl.mid, sl.turn).await else {
        println!("[live] retire {pane}: lost the take race, keeping");
        return;
    };
    delete_live(s, pane, taken).await;
}

/// Unconditional retire (genuine cancel / dead submit with no
/// successor): nobody else can own the turn — delete when auto-remove
/// is on, keep (slot included) when off. Idempotent.
pub async fn clear_live(s: &AppState, pane: &str) {
    if !s.transient_remove() {
        println!("[live] retire {pane}: kept (transient off)");
        return;
    }
    let Some(sl) = s.live_take(pane).await else {
        return;
    };
    delete_live(s, pane, sl).await;
}

/// Watcher-exit retire: only when no successor owns the pane — a live
/// job in the map keeps serving (its ticks own the shared slot), and a
/// live same-Arc turn (epoch moved without stopping) does too. A
/// stopped job never owns a successor turn (enqueue never reuses
/// stopped Arcs), so quiet pane-death retires still retire.
/// Idempotent with the finalize/cancel clears above.
pub async fn clear_live_if_ownerless(
    s: &AppState,
    pane: &str,
    job: &std::sync::Arc<crate::jobs::job::Job>,
    exit_epoch: u64,
) {
    // Another live Arc owns the pane and may still serve: hands off
    // (its ticks own the shared slot; retiring here would take a live
    // turn's canvas). A stopped entry owns nothing — fall through.
    if let Some(cur) = s.jobs.lock().await.get(pane).cloned()
        && !std::sync::Arc::ptr_eq(&cur, job)
        && !cur.is_stopped()
    {
        return;
    }
    // Live same-Arc successor turn (supersede bumps in place): hands off.
    if !job.is_stopped() && job.epoch.load(std::sync::atomic::Ordering::Relaxed) != exit_epoch {
        return;
    }
    clear_live(s, pane).await;
}
