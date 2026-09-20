//! Prompt result finalization: turn the live message into the final
//! card. Split from `runner` (300-line file limit). `watch_job` calls
//! `finalize` on settle; `enqueue_prompt` reports submit errors.
//! Stamps (baseline, done-mark, books) are epoch-gated: a submit landing
//! mid-RPC owns the pane, and the old prompt must stamp nothing.
use crate::{
    herdr::client::read_screen_adaptive,
    jobs::{
        arbitrate::select_final_body,
        books::settle_books,
        finalize_blocked::try_finalize_blocked,
        job::Job,
        report::{RUN_ENDED, fold_live, retire_live},
    },
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Turn the live message into the final result card: last-segment reply
/// arbitrated against one settled read (see select_final_body — herdr
/// exposes only raw TUI text, so answers ride the filtered stream).
/// True when nothing was delivered (outage or all parts failed) so the
/// watcher retries instead of retiring the intent.
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
            // Fold the live slot (retiring with it set orphans a frozen
            // card); address from the slot itself, never job.dest.
            fold_live(s, live_dest, live_mid, RUN_ENDED).await;
            // Gated like every stamp path: a submit racing the fold owns
            // the pane — anchoring our empty snapshot would eat the fresh
            // baseline its reply needs.
            if job.epoch.load(Ordering::Relaxed) == entry_epoch {
                s.seen.lock().await.insert(pane.to_string(), snapshot);
            }
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
    // Split to `finalize_blocked` (300-line file limit).
    if let Some(r) = try_finalize_blocked(
        s,
        pane,
        job,
        settled,
        live_mid,
        live_dest,
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
    // the span never resurfaces as a stale "fresh" delta. A live card
    // is retired, not orphaned frozen on "working…".
    if body.is_empty() {
        // Gate before observing: a submit racing the RPCs above owns
        // the pane — observe nothing, consume nothing (its handoff
        // retires the live slot; consuming here would land a stale edit
        // the cancel arm then double-posts).
        if job.epoch.load(Ordering::Relaxed) != entry_epoch {
            acc.clear();
            return false;
        }
        observe_status(s, pane, settled, true, "job").await;
        // Address from the slot (remap-safe); mid without dest drops
        // without editing (fail-closed, never wrong thread).
        // Bounded like the blocked-path retire: a slow Telegram must not
        // stall settle past the tick — take the slot only on landed/gone
        // (finalize_blocked parity): a timeout keeps the slot for the
        // next tick instead of orphaning a frozen card.
        if let Some(mid) = *live_mid {
            if let Some((lchat, _)) = *live_dest {
                let retire = tokio::time::timeout(
                    std::time::Duration::from_secs(crate::types::LIVE_RPC_TIMEOUT_SECS),
                    s.tg.try_edit_msg(lchat, mid, "✅ settled — no fresh output", None),
                )
                .await;
                let done = match retire {
                    Ok(Ok(())) => true,
                    Ok(Err(e)) => crate::telegram::messages::edit_gone(&e.to_string()),
                    Err(_) => false,
                };
                if done {
                    live_mid.take();
                    live_dest.take();
                    let _ = s.tg.set_reaction(lchat, mid, Some("✅")).await;
                }
            } else {
                live_mid.take();
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
    let parts = chunks(&body, MAX_MSG_UNITS);

    // Gate before observing/touching: a newer submit mid-post would
    // retarget the card and corrupt the status reflection.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        acc.clear();
        return false;
    }
    // Snapshot the dialog sig before the observe RPCs: a fresh `blocked`
    // episode stamping mid-window must survive below (generation-checked
    // remove — a blind remove would wipe it and repost a ghost card).
    let pre_sig = s.blocked_sig.lock().await.get(pane).cloned();
    observe_status(s, pane, settled, true, "job").await;
    let (chat, th) = *job.dest.lock().await;
    // And after: a submit during the observe RPCs above retargets.
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        println!("[prompt] finalize {pane}: superseded before post, dropping");
        acc.clear();
        return false;
    }
    // Leaving blocked state clears the dialog signature (blocked path
    // returns above, so this only runs for settled non-blocked).
    // Generation-checked: a new blocked episode stamped during the RPCs
    // above keeps its dedup, or its fresh question card double-buzzes.
    {
        let mut m = s.blocked_sig.lock().await;
        if m.get(pane) == pre_sig.as_ref() {
            m.remove(pane);
        }
    }
    println!(
        "[prompt] finalize {pane}: {} part(s), body {} chars",
        parts.len(),
        body.len()
    );
    // Finals buzz; progress stayed silent in place. The working card is
    // DELETED once a part lands (the reply is the tombstone — no "✅ done"
    // corpse beside it); a total failure keeps the slot for retry, a
    // mid-post supersede leaves it for the handoff's retire instead of
    // stranding a done stamp with no reply.
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
    // Known limit: a multi-send is not atomic (`delivered` is any-part,
    // so a mid-post outage truncates: missing tail, never silent loss).
    // Duplication happens only on total failure (retry re-posts from
    // part 0); per-call retries narrow the window.
    if !delivered {
        println!("[prompt] finalize {pane}: delivery failed, keeping intent for retry");
        return true;
    }
    // Working card retires only now that the finish landed: deleted,
    // with an in-place "✅ done" fold only when deletion fails.
    retire_live(s, live_dest, live_mid).await;
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
        s.seen.lock().await.insert(pane.to_string(), snapshot);
    }
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    false
}

pub use super::report::{edit_live, report, report_done};
