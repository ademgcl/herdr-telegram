//! Prompt result finalization: post the settled reply as the final
//! card. Split from `runner` (300-line file limit). `watch_job` calls
//! `finalize` on settle; `enqueue_prompt` reports submit errors.
//! Stamps (baseline, done-mark, books) are epoch-gated: a submit landing
//! mid-RPC owns the pane, and the old prompt must stamp nothing.
use crate::{
    herdr::client::read_screen_adaptive,
    jobs::{
        arbitrate::select_final_body, books::settle_books, finalize_blocked::try_finalize_blocked,
        job::Job,
    },
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Post the settled reply as the final result card: last-segment reply
/// arbitrated against one settled read (see select_final_body — herdr
/// exposes only raw TUI text, so answers ride the filtered stream).
/// True when nothing was delivered (outage or all parts failed) so the
/// watcher retries instead of retiring the intent.
#[allow(clippy::too_many_arguments)]
pub async fn finalize(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    acc: &mut Vec<String>,
    entry: (u64, usize, String),
) -> bool {
    // Settle-owned entry triple: snapshotted before the remap RPCs, never
    // re-read here — a cross-thread submit in the gap would else pair the
    // new epoch/prompt with this old acc/screen.
    let (entry_epoch, entry_pending, prompt) = entry;
    // One settled read, arbitrated against the stream (see
    // select_final_body): alt-screen TUIs starve the delta stream, so a
    // trivial fragment must not shadow the real answer. The same screen
    // doubles as the spontaneous baseline below (no second RPC).
    let mut screen = read_screen_adaptive(&s.cfg.socket, pane).await;
    let mut body = select_final_body(acc, &screen, &prompt);
    // TUI-lag race: status flips settled a beat before the frame
    // renders. Two delayed re-reads (~4s) rescue fast-task replies.
    if body.is_empty() {
        for _ in 0..2 {
            // Cancellable grace wait (settle.rs parity): a submit or
            // /cancel landing mid-wait owns the pane at once, never after
            // up to 4s of dead sleep.
            super::settle::sleep_or_superseded(job, entry_epoch, std::time::Duration::from_secs(2))
                .await;
            // Superseded during the grace wait: drop like any mid-finalize
            // retarget below instead of posting stale. Cancel counts too:
            // /cancel bumps epoch AND marks stopped — the mark covers a
            // retire that reused the epoch without bumping it.
            if job.epoch.load(Ordering::Relaxed) != entry_epoch || job.is_stopped() {
                println!("[prompt] finalize {pane}: superseded in grace wait, dropping");
                settle_books(s, pane, job, entry_epoch, entry_pending).await;
                acc.clear();
                return false;
            }
            screen = read_screen_adaptive(&s.cfg.socket, pane).await;
            body = select_final_body(acc, &screen, &prompt);
            if !body.is_empty() {
                break;
            }
        }
    }
    let snapshot = screen;
    // Nothing readable and nothing delivered: keep the intent for retry.
    // (A chrome-only `acc` over an empty snapshot is still an outage —
    // retiring here would eat the reply.)
    // Bound: gone panes (dead/closed/exited) never render again — retire
    // instead of retrying forever with no card ever posted.
    if body.is_empty() && snapshot.is_empty() {
        if matches!(settled, "dead" | "closed" | "exited") {
            println!("[prompt] finalize {pane}: pane gone with no output, retiring");
            // A submit racing the settle read retargets everything: the
            // epoch handoff below owns the books then, so retire only
            // for the prompt that is still current.
            if job.epoch.load(Ordering::Relaxed) != entry_epoch {
                settle_books(s, pane, job, entry_epoch, entry_pending).await;
                acc.clear();
                return false;
            }
            // Gone with no output: nothing was ever posted (no live card
            // exists to fold — see live.rs), just retire the books.
            // Empty never anchors (anchor parity with
            // Job::anchor_baseline): a blank read would wipe a good
            // baseline and repost scrollback as fresh on reuse.
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            return false;
        }
        println!("[prompt] finalize {pane}: read outage, keeping intent for retry");
        return true;
    }
    // Superseded mid-finalize (a new prompt landed during the settle
    // RPCs): post nothing and stamp nothing — stream, baseline and
    // destination all belong to the old prompt. Drop the stale
    // accumulation; the caller sees the epoch move and keeps serving
    // the new prompt, and settle_books preserves its books below.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded, dropping stale body");
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return false;
    }

    // Blocked settle: never the stream body (↳ echoes + footer would
    // pre-empt the question card) — always the blocked-card path.
    // Split to `finalize_blocked` (300-line file limit).
    if let Some(r) = try_finalize_blocked(
        s,
        pane,
        job,
        settled,
        snapshot.clone(),
        entry_epoch,
        entry_pending,
        acc,
    )
    .await
    {
        return r;
    }
    // Empty non-blocked settle: post nothing, but anchor the screen so
    // the span never resurfaces as a stale "fresh" delta (no live card
    // exists to retire — see live.rs).
    // Split to `finalize_blocked::try_finalize_empty` (300-line file limit).
    if body.is_empty()
        && let Some(r) = super::finalize_blocked::try_finalize_empty(
            s,
            pane,
            job,
            settled,
            snapshot.clone(),
            entry_epoch,
            entry_pending,
            acc,
        )
        .await
    {
        return r;
    }
    let parts = chunks(&body, MAX_MSG_UNITS);

    // Gate before observing/touching: a newer submit mid-post would
    // retarget the card and corrupt the status reflection.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return false;
    }
    // Snapshot the dialog sig before the observe RPCs: a fresh `blocked`
    // episode stamping mid-window must survive below (generation-checked
    // remove — a blind remove would wipe it and repost a ghost card).
    let pre_sig = s.blocked_sig.lock().await.get(pane).cloned();
    // Re-gate after the snapshot await: a submit during it owns the pane
    // — the observe below would stamp the old settled kind over the new
    // turn's live status (backwards transition + stale debounce arm).
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return false;
    }
    observe_status(s, pane, settled, true, "job").await;
    let (chat, th) = *job.dest.lock().await;
    // And after: a submit during the observe RPCs above retargets.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        acc.clear();
        return false;
    }
    // Leaving blocked state clears the dialog signature (blocked path
    // returns above, so this only runs for settled non-blocked).
    // Generation-checked + epoch re-checked INSIDE the lock: a submit
    // between the check above and this lock would else wipe the
    // successor's fresh stamp when it equals pre_sig (same question).
    {
        let mut m = s.blocked_sig.lock().await;
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            drop(m);
            println!("[prompt] finalize {pane}: superseded before post, dropping");
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            acc.clear();
            return false;
        }
        if m.get(pane) == pre_sig.as_ref() {
            m.remove(pane);
        }
    }
    println!(
        "[prompt] finalize {pane}: {} part(s), body {} chars",
        parts.len(),
        body.len()
    );
    // Finals buzz; progress stayed silent in place (no working card was
    // ever posted — see live.rs). A total failure keeps the slot for
    // retry, a mid-post supersede leaves delivered heads for the
    // best-effort cleanup below instead of stranding a done stamp with
    // no reply.
    let mut delivered = false;
    let mut landed: Vec<i64> = Vec::new();
    for part in parts.iter() {
        if let Some(m) = super::report::report_done_mid(s, chat, th, pane, part).await {
            delivered = true;
            landed.push(m);
        }
        // Retarget check per part: a slow flood-wait can span a submit.
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            println!("[prompt] finalize {pane}: superseded mid-post, stopping");
            // Delete delivered heads best-effort (post-send guard
            // parity above): the new prompt owns the thread — a stale
            // head beside its turn misattributes the reply.
            for m in landed {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(crate::types::LIVE_RPC_TIMEOUT_SECS),
                    s.tg.delete_msg(chat, m),
                )
                .await;
            }
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            acc.clear();
            return false;
        }
    }
    // Total delivery failure: keep the intent and retry like a read
    // outage — retiring here would lose the reply with no re-arm.
    // Known limit: a multi-send is not atomic (`delivered` is any-part,
    // so a mid-post outage truncates: missing tail, never silent loss).
    // Duplication happens only on total failure (retry re-posts from
    // part 0); per-call retries narrow the window.
    if !delivered {
        println!("[prompt] finalize {pane}: delivery failed, keeping intent for retry");
        return true;
    }
    // Stamp the prompt completion so the notifier can suppress the
    // redundant post-prompt idle/done echo (the card already answered),
    // and anchor the spontaneous baseline so this card is never reposted.
    // Epoch-gated like the blocked/empty paths: a submit landing during
    // the fold owns the pane — anchoring its fresh output away into our
    // stale snapshot would eat the reply.
    if job.epoch.load(Ordering::Relaxed) == entry_epoch {
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
        // Empty/all-blank never anchors (anchorable_screen parity with
        // Job::anchor_baseline): a blank settled read over a streamed
        // `acc` body would wipe a good baseline and repost scrollback
        // as fresh on reuse.
        if crate::types::anchorable_screen(&snapshot) {
            s.seen.lock().await.insert(pane.to_string(), snapshot);
        }
    }
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    false
}

pub use super::report::report;
