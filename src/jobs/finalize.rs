//! Prompt result finalization: turn the live message into the final
//! card. Split from `runner` (300-line file limit). `watch_job` calls
//! `finalize` on settle; `enqueue_prompt` reports submit errors.
//! Stamps (baseline, done-mark, books) are epoch-gated: a submit landing
//! mid-RPC owns the pane, and the old prompt must stamp nothing.
use crate::{
    handlers::dialog::send_blocked_card,
    herdr::client::read_screen_adaptive,
    jobs::{
        arbitrate::select_final_body,
        books::settle_books,
        job::Job,
        report::{RUN_ENDED, fold_live},
    },
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Turn the live message into the final result card from the generic,
/// provider-agnostic screen stream, arbitrated against one settled
/// read (see select_final_body).
/// (herdr exposes only raw TUI text for every provider — no clean-text
/// API — so answers ride on the chrome-filtered stream, never raw tails.)
/// Returns true when nothing was delivered (read outage OR every card
/// part failed to send) so the watcher loop retries instead of retiring
/// the intent.
pub async fn finalize(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    live_mid: &mut Option<i64>,
    live_dest: &mut Option<(i64, Option<i64>)>,
    acc: &mut Vec<String>,
) -> bool {
    // Fresh reply only: the last segment after tool calls, reasoning
    // headers and the prompt echo — earlier turns and intermediate work
    // are dropped. Falls back to the settled screen for fast tasks where
    // nothing streamed.
    let entry_epoch = job.epoch.load(Ordering::Relaxed);
    let entry_pending = *job.pending.lock().await;
    let prompt = job.prompt.lock().await.clone();
    // One settled read, arbitrated against the stream (see
    // select_final_body): alt-screen TUIs starve the delta stream, so a
    // trivial fragment must not shadow the real answer. The same screen
    // doubles as the spontaneous baseline below (no second RPC).
    let mut screen = read_screen_adaptive(&s.cfg.socket, pane).await;
    let mut body = select_final_body(acc, &screen, &prompt);
    // TUI-lag race: status flips settled a beat before the frame
    // renders the answer. Two delayed re-reads (~4s) rescue fast-task
    // replies; lag beyond that needs a fresh transition (vanishingly
    // rare next to stalling every empty settle).
    if body.is_empty() {
        for _ in 0..2 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            // Superseded during the grace wait: drop like any mid-finalize
            // retarget below instead of posting stale.
            if job.epoch.load(Ordering::Relaxed) != entry_epoch {
                println!("[prompt] finalize {pane}: superseded in grace wait, dropping");
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
            // epoch handoff below owns the live slot then, so fold only
            // for the prompt that is still current.
            if job.epoch.load(Ordering::Relaxed) != entry_epoch {
                acc.clear();
                return false;
            }
            s.seen.lock().await.insert(pane.to_string(), snapshot);
            // Fold the live slot: retiring with it set would orphan a
            // frozen "working…" card (the caller only drops the address
            // when the slot is already consumed). Address comes from the
            // live slot itself, never job.dest (remap race).
            fold_live(s, live_dest, live_mid, RUN_ENDED).await;
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
        acc.clear();
        return false;
    }

    // Blocked settle: never the stream body (↳ echoes + footer would
    // pre-empt the question card) — always the blocked-card path.
    if settled == "blocked" {
        // Boot seed already carded this exact dialog: observe + books
        // only, or the same question buzzes twice with ❗.
        if s.blocked_sig.lock().await.get(pane)
            .map(|v| v == &crate::handlers::dialog::dialog_sig(&snapshot))
            .unwrap_or(false)
        {
            observe_status(s, pane, settled, true, "job").await;
            // Any live slot duplicates the already-buzzed question card:
            // fold it whether it holds streamed output or only the silent
            // ack, or the working card freezes next to the question.
            fold_live(
                s,
                live_dest,
                live_mid,
                "⛔ blocked — see question card",
            )
            .await;
            settle_books(s, pane, job, entry_epoch, entry_pending).await;
            return false;
        }
        let (chat, th) = *job.dest.lock().await;
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
            return false;
        }
        // Undelivered: retry while there is somewhere to post (a pruned
        // topic mapping means the card can never land — retire instead
        // of spinning forever).
        let mappable = s.cfg.forum.is_none() || s.topics.all_mappings().contains_key(pane);
        if mappable {
            println!("[prompt] finalize {pane}: blocked card undelivered, retrying");
            return true;
        }
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return false;
    }
    // Empty non-blocked settle: post nothing, but anchor the evaluated
    // screen so the span doesn't rot in the baseline and resurface as a
    // stale "fresh" delta on the next transition (the spontaneous path
    // re-evaluates from here and stays quiet on no change). A live card
    // is retired, not orphaned frozen on "working…".
    if body.is_empty() {
        observe_status(s, pane, settled, true, "job").await;
        // Address from the slot (remap-safe); mid without dest drops
        // without editing (fail-closed, never wrong thread).
        if let Some(mid) = live_mid.take() {
            if let Some((lchat, _)) = live_dest.take() {
                s.tg.edit_msg(lchat, mid, "✅ settled — no fresh output", None)
                    .await;
                let _ = s.tg.set_reaction(lchat, mid, Some("✅")).await;
            }
        } else {
            live_dest.take();
        }
        // Superseded during the RPCs above: stamp nothing (blocked-path rule).
        if job.epoch.load(Ordering::Relaxed) == entry_epoch {
            s.seen.lock().await.insert(pane.to_string(), snapshot);
        }
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return false;
    }
    let text = body.clone();
    let parts = chunks(&text, MAX_MSG_UNITS);

    observe_status(s, pane, settled, true, "job").await;
    // Leaving blocked state clears the dialog signature (blocked path
    // returns above, so this only runs for settled non-blocked).
    s.blocked_sig.lock().await.remove(pane);
    let (chat, th) = *job.dest.lock().await;
    // A newer submit mid-post would retarget the card: re-check before
    // touching Telegram or stamping anything.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        acc.clear();
        return false;
    }
    println!(
        "[prompt] finalize {pane}: {} part(s), body {} chars",
        parts.len(),
        body.len()
    );
    // Finals buzz; progress stayed silent in place. Fold the working
    // card only after a part lands — a total failure keeps the slot for
    // retry, a mid-post supersede leaves it for the handoff's retire
    // instead of stranding a "✅ done" corpse with no reply.
    let mut delivered = false;
    for part in parts.iter() {
        if report_done(s, chat, th, pane, part).await {
            delivered = true;
        }
        // Retarget check per part: a slow flood-wait can span a submit.
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            println!("[prompt] finalize {pane}: superseded mid-post, stopping");
            acc.clear();
            return false;
        }
    }
    // Total delivery failure: keep the intent and retry like a read
    // outage — retiring here would lose the reply with no re-arm.
    // (Stamps below describe a card the user saw; failed posts stamp
    // nothing, so the retry re-posts from an intact baseline.)
    // Known limit: a multi-send is not atomic. `delivered` is any-part,
    // so an outage/revocation landing mid-post truncates and retires:
    // the landed prefix posts with no explicit truncation marker
    // (missing tail, never silent loss of the whole reply). Duplication
    // happens only on total failure (nothing landed): the retry re-posts
    // from part 0. Skipping landed parts instead would risk the opposite
    // (a failed send that actually landed goes missing with no trace).
    // Per-call retries (3 sends + 3 flood-waits) narrow the window.
    if !delivered {
        println!("[prompt] finalize {pane}: delivery failed, keeping intent for retry");
        return true;
    }
    // Working card retires only now that the finish landed.
    fold_live(s, live_dest, live_mid, "✅ done").await;
    // Stamp the prompt completion so the notifier can suppress the
    // redundant post-prompt idle/done echo (the card already answered),
    // and anchor the spontaneous baseline so this card is never reposted.
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());
    s.seen.lock().await.insert(pane.to_string(), snapshot);
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    false
}

pub use super::report::{edit_live, report, report_done};
