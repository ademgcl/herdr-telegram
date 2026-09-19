//! Blocked-settle arm: split from `finalize` (300-line file limit).
//! Never the stream body — always the blocked-card path (single-flight
//! with tap answers, contention silent).
use crate::{
    handlers::dialog::{dialog_sig, send_blocked_card},
    jobs::{books::settle_books, job::Job, report::fold_live},
    notifier::observe_status,
    state::AppState,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Handle the `blocked` settle. `Some(retry)` when handled, `None` when
/// `settled != "blocked"` (caller falls through to empty/body arms).
/// True = keep intent for retry (outage/undelivered), false = retire.
#[allow(clippy::too_many_arguments)]
pub async fn try_finalize_blocked(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    snapshot: Vec<String>,
    entry_epoch: u64,
    entry_pending: usize,
    acc: &mut Vec<String>,
) -> Option<bool> {
    if settled != "blocked" {
        return None;
    }
    // Boot seed already carded this exact dialog: observe + books
    // only, or the same question buzzes twice with ❗.
    if s.blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == &dialog_sig(&snapshot))
        .unwrap_or(false)
    {
        // Superseded mid-RPCs: observe nothing stale.
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            acc.clear();
            return Some(false);
        }
        observe_status(s, pane, settled, true, "job").await;
        // Any live slot duplicates the already-buzzed question card:
        // fold its streamed output, or the working card freezes next
        // to the question.
        fold_live(s, live_dest, live_mid, crate::ui::BLOCKED_SEE_CARD).await;
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    }
    let (chat, th) = *job.dest.lock().await;
    // Superseded mid-RPCs: post/consume nothing (the handoff retires
    // the live slot; consuming it strands a stale ❗ with no owner).
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        acc.clear();
        return Some(false);
    }
    // Retire the live working card in place (address from the slot,
    // never job.dest): the question card posts fresh below.
    // Fail-closed: mid without dest drops without editing rather
    // than guessing the thread after a remap.
    if let Some(mid) = live_mid.take() {
        if let Some((lchat, _)) = live_dest.take() {
            s.tg.edit_msg(
                lchat,
                mid,
                "⛔ blocked — needs input (see next message)",
                None,
            )
            .await;
        }
    } else {
        live_dest.take();
    }
    // Re-check after the retire edit: a submit during it owns the
    // pane — the ❗ card below would buzz stale beside its prompt.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        acc.clear();
        return Some(false);
    }
    // Single-flight with tap answers: a concurrent tap that already
    // passed its sig check owns the card — contention stays silent
    // (no strip, no second buzz), the holder's card stands. Books still
    // settle: every other arm does, or pending-count/intent leaks and
    // replays after restart.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    };
    // Re-check after the claim: the tap winner may have just stamped
    // this exact dialog — posting again would double-buzz ❗.
    if s.blocked_sig
        .lock()
        .await
        .get(pane)
        .map(|v| v == &dialog_sig(&snapshot))
        .unwrap_or(false)
    {
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            acc.clear();
            return Some(false);
        }
        observe_status(s, pane, settled, true, "job").await;
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    }
    let posted = send_blocked_card(s, chat, th, pane).await;
    // Silent icon sync (later observations dedupe via blocked_sig).
    observe_status(s, pane, settled, true, "job").await;
    if posted {
        // A submit landing during the card post owns the pane now:
        // stamp nothing, or the new prompt's fresh output anchors
        // away into the old prompt's baseline.
        if job.epoch.load(Ordering::Relaxed) == entry_epoch {
            s.last_done
                .lock()
                .await
                .insert(pane.to_string(), std::time::Instant::now());
            // Anchor the baseline so later settles don't repost the dialog.
            s.seen.lock().await.insert(pane.to_string(), snapshot);
        }
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    }
    // Undelivered: retry while there is somewhere to post (a pruned
    // topic mapping means the card can never land — retire instead
    // of spinning forever).
    let mappable = s.cfg.forum.is_none() || s.topics.all_mappings().contains_key(pane);
    if mappable {
        println!("[prompt] finalize {pane}: blocked card undelivered, retrying");
        return Some(true);
    }
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    Some(false)
}
