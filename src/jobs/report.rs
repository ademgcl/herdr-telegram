//! Delivery helpers for live prompts and reports. Split from finalize.rs.
use crate::{state::AppState, types::LIVE_RPC_TIMEOUT_SECS};

/// Quiet retire text: never freeze a live "working…" card.
pub const RUN_ENDED: &str = "⏹️ run ended";
/// User-cancel text: cancel branches edit the live card in place, posting
/// fresh only when no card ever streamed (nothing to edit) or the card is
/// definitely gone — never on a transient failure (that would orphan a
/// frozen card with no retry, or duplicate like the stream path refuses).
pub const CANCELLED: &str = "✋ cancelled";

/// Cancel ownership verdict (pure, tested): a superseding enqueue owns
/// the intent — a stale watcher's retire clears it only when the jobs
/// map still points here. Single source for `runner::cancel_watch`.
pub(crate) fn cancel_owns_intent(
    current: Option<&std::sync::Arc<super::job::Job>>,
    job: &std::sync::Arc<super::job::Job>,
) -> bool {
    current.is_some_and(|j| std::sync::Arc::ptr_eq(j, job))
}

/// Cancel edit: retires the live card where it lives (live_dest), never
/// job.dest (remap race). Falls back to a fresh post at the current dest
/// only when the card is definitely gone — a transient edit failure
/// keeps the slot for the next turn to adopt (a taken slot would orphan
/// a frozen card with no retry), never posts fresh (that would
/// orphan-duplicate like the stream path refuses to).
pub async fn edit_live(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    text: &str,
) {
    let Some(mid) = *live_mid else {
        live_dest.take();
        report(s, chat_id, thread_id, pane, text).await;
        return;
    };
    // Fail-closed: mid without dest cannot happen (set together);
    // drop without editing rather than guessing job.dest and
    // clobbering the wrong thread after a remap.
    let Some((lchat, _)) = *live_dest else {
        live_mid.take();
        return;
    };
    // Bounded like every other live-card edit (live.rs, repoint,
    // finalize): try_edit_msg sleeps on flood-wait, which would park the
    // watcher mid-exit. A timeout is transient — the slot stays for the
    // next turn to adopt, exactly like any other non-gone error.
    let edit = tokio::time::timeout(
        std::time::Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
        s.tg.try_edit_msg(lchat, mid, text, None),
    )
    .await;
    match edit {
        Ok(Ok(())) => {
            live_mid.take();
            live_dest.take();
        }
        Ok(Err(e)) if crate::telegram::messages::edit_gone(&e.to_string()) => {
            live_mid.take();
            live_dest.take();
            report(s, chat_id, thread_id, pane, text).await;
        }
        Ok(Err(_)) | Err(_) => {}
    }
}

pub async fn report(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> bool {
    send_remembered(s, chat_id, thread_id, pane, msg)
        .await
        .is_some()
}

/// Final-card part delivery with ✅ reaction: shared by `finalize`'s
/// multi-part post (moved here from `finalize` under the file limit).
pub async fn report_done(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> bool {
    match send_remembered(s, chat_id, thread_id, pane, msg).await {
        Some(m) => {
            let _ = s.tg.set_reaction(chat_id, m, Some("✅")).await;
            true
        }
        None => false,
    }
}

/// Send + delivery-track, without any reaction: `report` (plain cards)
/// and `report_done` (✅ finals) share it so failure logging and intent
/// tracking can never drift between the two.
async fn send_remembered(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    msg: &str,
) -> Option<i64> {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    if mid.is_none() {
        eprintln!("[prompt] delivery failed {pane} (thread {thread_id:?})");
        return None;
    }
    s.remember(chat_id, mid, pane).await;
    mid
}

/// Working-card retire after the final lands: DELETE it so no "✅ done"
/// corpse buzzes beside the reply (the final is the tombstone). Falls
/// back to the in-place fold when deletion fails (lost rights, gone
/// thread) — never a frozen "working…" card. Address from the slot,
/// never job.dest (remap race), same as `fold_live`.
pub async fn retire_live(
    s: &AppState,
    live_dest: &mut Option<(i64, Option<i64>)>,
    live_mid: &mut Option<i64>,
) {
    if let Some(mid) = live_mid.take() {
        if let Some((chat, _)) = live_dest.take() {
            if s.tg.delete_msg(chat, mid).await {
                return;
            }
            let _ = s.tg.try_edit_msg(chat, mid, "✅ done", None).await;
        }
    } else {
        live_dest.take();
    }
}

/// Best-effort fold of a live card with NO fallback post: quiet retire
/// paths (dead pane / shell flip) must never buzz a new message — the
/// frozen "working…" card just resolves in place, or stays if the
/// thread is already gone (edit fails silently). Take the slot only on
/// landed/gone (live.rs retire_for_handoff parity): a transient failure
/// keeps it for the next turn to adopt instead of freezing the old card.
pub async fn fold_live(
    s: &AppState,
    live_dest: &mut Option<(i64, Option<i64>)>,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    let Some(mid) = *live_mid else {
        live_dest.take();
        return;
    };
    let Some((chat, _)) = *live_dest else {
        live_mid.take();
        return;
    };
    // Bounded like edit_live above: an unbounded fold parks the watcher
    // at its loop top on flood-wait. Timeout keeps the slot (transient).
    let edit = tokio::time::timeout(
        std::time::Duration::from_secs(LIVE_RPC_TIMEOUT_SECS),
        s.tg.try_edit_msg(chat, mid, text, None),
    )
    .await;
    match edit {
        Ok(Ok(())) => {
            live_mid.take();
            live_dest.take();
        }
        Ok(Err(e)) if crate::telegram::messages::edit_gone(&e.to_string()) => {
            live_mid.take();
            live_dest.take();
        }
        Ok(Err(_)) | Err(_) => {}
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
