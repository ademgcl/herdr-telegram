//! Blocked-settle arm: split from `finalize` (300-line file limit).
//! Never the stream body — always the blocked-card path (single-flight
//! with tap answers, contention silent).
use crate::{
    handlers::dialog::{dialog_sig, send_blocked_card},
    jobs::{books::settle_books, job::Job},
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
    snapshot: Vec<String>,
    entry_epoch: u64,
    entry_pending: usize,
    live_entry: Option<(i64, u64)>,
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
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            acc.clear();
            return Some(false);
        }
        observe_status(s, pane, settled, true, "job").await;
        // The question already buzzed elsewhere: retire the transient
        // with the books (generation-gated — a successor owns it now).
        super::progress::clear_live_if_epoch(s, pane, live_entry).await;
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    }
    let (chat, th) = *job.dest.lock().await;
    // Superseded mid-RPCs: post/consume nothing (a newer prompt owns
    // the pane now — posting here would strand a stale ❗ with no owner).
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return Some(false);
    }
    // Single-flight with tap answers: a concurrent tap that already
    // passed its sig check owns the card — contention stays silent
    // (no strip, no second buzz). Intent is KEPT for retry, never
    // settled here: nothing proves the holder's card landed, and
    // retiring on assumption silently loses the question when the
    // holder's send drops. The next tick retires via the sig-match arm
    // above once the holder stamps, or wins the claim and posts itself.
    let Some(_op) = crate::state::OpGuard::claim(&s.blockop, pane).await else {
        return Some(true);
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
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            acc.clear();
            return Some(false);
        }
        observe_status(s, pane, settled, true, "job").await;
        super::progress::clear_live_if_epoch(s, pane, live_entry).await;
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return Some(false);
    }
    let posted = {
        // Remap-safe (stall.rs parity): a paced reset migrating the
        // topic during the RPCs above must not buzz into the deleted
        // thread — follow the live mapping at send time. Unmapped
        // forum dests retire: there is nowhere to post (mappable rule
        // below would spin retry forever against a deleted topic).
        let th = if s.cfg.forum == Some(chat) {
            match s.topics.storage.get_thread(pane) {
                Some(cur) => Some(cur),
                None => {
                    settle_books(s, pane, job, entry_epoch, entry_pending).await;
                    acc.clear();
                    return Some(false);
                }
            }
        } else {
            th
        };
        send_blocked_card(s, chat, th, pane).await
    };
    // Re-gate after the post await: a submit during it owns the pane —
    // the observe below would stamp the old settled kind over the new
    // turn's live status (finalize.rs parity). Books still cover our
    // entry share.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return Some(false);
    }
    // Silent icon sync (later observations dedupe via blocked_sig).
    observe_status(s, pane, settled, true, "job").await;
    if posted {
        // A submit landing during the card post owns the pane now:
        // stamp nothing, or the new prompt's fresh output anchors
        // away into the old prompt's baseline.
        if job.epoch.load(Ordering::Relaxed) == entry_epoch {
            // The buzzing question card landed: the silent transient
            // retires with it (the card is the final word on this turn).
            super::progress::clear_live_if_epoch(s, pane, live_entry).await;
            s.last_done
                .lock()
                .await
                .insert(pane.to_string(), std::time::Instant::now());
            // Anchor the baseline so later settles don't repost the
            // dialog — never an all-blank read (anchorable_screen
            // parity): it would wipe a good baseline and repost
            // scrollback as fresh.
            if crate::types::anchorable_screen(&snapshot) {
                s.seen.lock().await.insert(pane.to_string(), snapshot);
            }
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

/// Empty non-blocked settle arm (split from `finalize`, 300-line file
/// limit): post nothing, but anchor the screen so the span never
/// resurfaces as a stale "fresh" delta. `Some(retry)` when handled.
#[allow(clippy::too_many_arguments)]
pub async fn try_finalize_empty(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    snapshot: Vec<String>,
    entry_epoch: u64,
    entry_pending: usize,
    live_entry: Option<(i64, u64)>,
    acc: &mut Vec<String>,
) -> Option<bool> {
    // Gate before observing: a submit racing the RPCs above owns
    // the pane — observe nothing, consume nothing (its watcher serves
    // the new prompt; consuming here would anchor its fresh output
    // away into our stale snapshot).
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return Some(false);
    }
    observe_status(s, pane, settled, true, "job").await;
    // Nothing to buzz for an empty settle — the transient retires
    // per the auto-remove flag (kept turns reuse it next).
    super::progress::clear_live_if_epoch(s, pane, live_entry).await;
    // Superseded during the RPCs above: stamp nothing (blocked-path rule).
    if job.epoch.load(Ordering::Relaxed) == entry_epoch
        && crate::types::anchorable_screen(&snapshot)
    {
        s.seen.lock().await.insert(pane.to_string(), snapshot);
    }
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    Some(false)
}
