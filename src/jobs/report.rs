//! Delivery helpers for live prompts and reports. Split from finalize.rs.
use crate::state::AppState;

/// Live-card head: the streaming card shows fresh output under it —
/// instant feedback before any output lands is the typing indicator,
/// never an empty card (a content-free "working…" row is a message
/// about nothing).
pub const WORKING_HEAD: &str = "🔄 working…";
/// Quiet retire text: never freeze a live "working…" card.
pub const RUN_ENDED: &str = "⏹️ run ended";
/// User-cancel text: every cancel branch edits in place, never posts fresh.
pub const CANCELLED: &str = "✋ cancelled";

/// Cancel edit: retires the live card where it lives (live_dest), never
/// job.dest (remap race). Falls back to a fresh post at the current dest
/// only when the card is definitely gone — a transient edit failure
/// posts nothing (a fresh send then would orphan-duplicate like the
/// stream path refuses to), the frozen card heals on the next turn.
pub async fn edit_live(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        // Fail-closed: mid without dest cannot happen (set together);
        // drop without editing rather than guessing job.dest and
        // clobbering the wrong thread after a remap.
        let Some((lchat, _)) = live_dest.take() else {
            return;
        };
        match s.tg.try_edit_msg(lchat, mid, text, None).await {
            Ok(()) => {}
            Err(e) if crate::telegram::messages::edit_gone(&e.to_string()) => {
                report(s, chat_id, thread_id, pane, text).await;
            }
            Err(_) => {}
        }
    } else {
        live_dest.take();
        report(s, chat_id, thread_id, pane, text).await;
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

/// Best-effort fold of a live card with NO fallback post: quiet retire
/// paths (dead pane / shell flip) must never buzz a new message — the
/// frozen "working…" card just resolves in place, or stays if the
/// thread is already gone (edit fails silently).
pub async fn fold_live(
    s: &AppState,
    live_dest: &mut Option<(i64, Option<i64>)>,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        if let Some((chat, _)) = live_dest.take() {
            let _ = s.tg.try_edit_msg(chat, mid, text, None).await;
        }
    } else {
        live_dest.take();
    }
}
