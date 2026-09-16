use crate::{
    handlers::dialog::send_blocked_card,
    herdr::client::read_screen_adaptive,
    jobs::{arbitrate::select_final_body, job::Job},
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};
/// Prompt result finalization: turn the live message into the final card.
/// Split from `runner` (300-line file limit). `watch_job` calls
/// `finalize` on settle; `enqueue_prompt` reports submit errors.
/// Returns true when nothing was delivered (read outage OR every card
/// part failed to send) so the watcher loop retries instead of retiring
/// the intent.
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Turn the live message into the final result card from the generic,
/// provider-agnostic screen stream, arbitrated against one settled
/// read (see select_final_body).
/// (herdr exposes only raw TUI text for every provider — no clean-text
/// API — so answers ride on the chrome-filtered stream, never raw tails.)
pub async fn finalize(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    live_mid: &mut Option<i64>,
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
    // TUI-lag race: the status flipped to settled a beat before the
    // frame rendered the answer. One short delayed re-read (not the
    // 5-60s outage backoff) rescues fast-task replies that would else
    // post "no fresh output" and anchor away the real answer.
    if body.is_empty() {
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
            s.seen.lock().await.insert(pane.to_string(), snapshot);
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

    // Blocked settle: the viewport holds the question, the scrollback
    // tail holds tool activity — so NEVER dump the stream body here (it
    // posts ↳ echoes + a reply footer ahead of the real question card).
    // Always take the blocked-card path: it reads the visible screen
    // (question text survives, activity doesn't) with answer buttons.
    if settled == "blocked" {
        let (chat, th) = *job.dest.lock().await;
        // Reuse the live message slot when there is one.
        if let Some(mid) = live_mid.take() {
            s.tg.edit_msg(
                chat,
                mid,
                "⛔ blocked — needs input (see next message)",
                None,
            )
            .await;
        }
        let posted = send_blocked_card(s, chat, th, pane).await;
        // Silent icon sync (later observations dedupe via blocked_sig).
        observe_status(s, pane, settled, true, "job").await;
        if posted {
            s.last_done
                .lock()
                .await
                .insert(pane.to_string(), std::time::Instant::now());
            // Anchor the baseline so later settles don't repost the dialog.
            s.seen.lock().await.insert(pane.to_string(), snapshot);
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
        if let Some(mid) = live_mid.take() {
            let (chat, _) = *job.dest.lock().await;
            s.tg.edit_msg(chat, mid, "✅ settled — no fresh output", None)
                .await;
        }
        s.seen.lock().await.insert(pane.to_string(), snapshot);
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
    let mut delivered = false;
    for (i, part) in parts.iter().enumerate() {
        match (i, *live_mid) {
            (0, Some(mid)) => {
                if s.tg.try_edit_msg(chat, mid, part, None).await.is_ok()
                    || report(s, chat, th, pane, part).await
                {
                    delivered = true;
                }
            }
            _ => {
                if report(s, chat, th, pane, part).await {
                    delivered = true;
                }
            }
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
    if !delivered {
        println!("[prompt] finalize {pane}: delivery failed, keeping intent for retry");
        return true;
    }
    // Stamp the prompt completion so the notifier can suppress the
    // redundant post-prompt idle/done echo (the card already answered),
    // and anchor the spontaneous baseline so this card is never reposted.
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());
    s.seen.lock().await.insert(pane.to_string(), snapshot);
    *live_mid = None;
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
    false
}

/// Cover the prompts owed at entry. A new submit mid-finalize bumps the
/// epoch: leave its pending count, persisted intent and map entry so the
/// watcher loop keeps serving it.
async fn settle_books(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    entry_epoch: u64,
    entry_pending: usize,
) {
    if job.epoch.load(Ordering::Relaxed) != entry_epoch {
        let mut p = job.pending.lock().await;
        *p = p.saturating_sub(entry_pending);
        return;
    }
    *job.pending.lock().await = 0;
    s.clear_pending(pane).await;
    let mut map = s.jobs.lock().await;
    if map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
        map.remove(pane);
        println!("[prompt] watcher retired: {pane}");
    }
}

pub async fn edit_live(
    s: &AppState,
    chat_id: i64,
    thread_id: Option<i64>,
    pane: &str,
    live_mid: &mut Option<i64>,
    text: &str,
) {
    if let Some(mid) = live_mid.take() {
        if s.tg.try_edit_msg(chat_id, mid, text, None).await.is_err() {
            report(s, chat_id, thread_id, pane, text).await;
        }
    } else {
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
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    if mid.is_none() {
        eprintln!("[prompt] delivery failed {pane} (thread {thread_id:?})");
        return false;
    }
    s.remember(chat_id, mid, pane).await;
    true
}
