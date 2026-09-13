/// Prompt result finalization: turn the live message into the final card.
/// Split from `runner` (300-line file limit). `watch_job` calls
/// `finalize` on settle; `enqueue_prompt` reports submit errors.
use std::sync::Arc;
use crate::{
    handlers::interactive::send_blocked_card,
    herdr::client::read_screen,
    jobs::job::Job,
    jobs::segment::final_block,
    jobs::stream::join_trimmed,
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};

/// Turn the live message into the final result card from the generic,
/// provider-agnostic screen stream: accumulated deltas first, then one
/// cleaned settled-screen fallback for fast tasks where nothing streamed.
/// (herdr exposes only raw TUI text for every provider — no clean-text
/// API — so answers ride on the chrome-filtered stream, never raw tails.)
pub async fn finalize(
    s: &AppState,
    pane: &str,
    job: &Arc<Job>,
    settled: &str,
    live_mid: &mut Option<i64>,
    acc: &mut Vec<String>,
) {
    // Fresh reply only: the last segment after tool calls, reasoning
    // headers and the prompt echo — earlier turns and intermediate work
    // are dropped. Falls back to the settled screen for fast tasks where
    // nothing streamed.
    let prompt = job.prompt.lock().await.clone();
    let seg = final_block(acc, &prompt);
    let (body, snapshot) = if !seg.is_empty() {
        (join_trimmed(&seg), None)
    } else {
        let screen = read_screen(&s.cfg.socket, pane, 80).await;
        let body = join_trimmed(&final_block(&screen, &prompt));
        (body, Some(screen))
    };

    // The card is the answer itself — never a status-word lead. Blocked
    // keeps the reply affordance. A blocked settle with no captured text
    // means an interactive prompt is up: post its answer card instead of
    // a dead "(no captured output)".
    if body.is_empty() && settled == "blocked" {
        let (chat, th) = *job.dest.lock().await;
        // Reuse the live message slot when there is one.
        if let Some(mid) = live_mid.take() {
            s.tg.edit_msg(chat, mid, "⛔ blocked — needs input (see next message)", None).await;
        }
        send_blocked_card(&s, chat, th, pane).await;
        s.last_done
            .lock()
            .await
            .insert(pane.to_string(), std::time::Instant::now());
        // Anchor the baseline so later settles don't repost the dialog.
        let snap = match snapshot {
            Some(snap) => snap,
            None => read_screen(&s.cfg.socket, pane, 80).await,
        };
        s.seen.lock().await.insert(pane.to_string(), snap);
        *job.pending.lock().await = 0;
        let mut map = s.jobs.lock().await;
        if map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false) {
            map.remove(pane);
        }
        return;
    }
    let text = if body.is_empty() {
        "(no captured output)".to_string()
    } else if settled == "blocked" {
        format!("{body}\n↩️ reply or type in topic to answer")
    } else {
        body.clone()
    };
    let parts = chunks(&text, MAX_MSG_UNITS);

    observe_status(s, pane, settled, true, "job").await;
    // Stamp the prompt completion so the notifier can suppress the
    // redundant post-prompt idle/done echo (the card already answered),
    // and anchor the spontaneous baseline so this card is never reposted.
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());
    let snap = match snapshot {
        Some(snap) => snap,
        None => read_screen(&s.cfg.socket, pane, 80).await,
    };
    s.seen.lock().await.insert(pane.to_string(), snap);
    let (chat, th) = *job.dest.lock().await;
    println!("[prompt] finalize {pane}: {} part(s), body {} chars", parts.len(), body.len());
    for (i, part) in parts.iter().enumerate() {
        match (i, *live_mid) {
            (0, Some(mid)) => s.tg.edit_msg(chat, mid, part, None).await,
            _ => report(s, chat, th, pane, part).await,
        }
    }
    *live_mid = None;
    *job.pending.lock().await = 0;

    // Nothing outstanding? Retire the watcher atomically.
    let mut map = s.jobs.lock().await;
    if *job.pending.lock().await == 0
        && map.get(pane).map(|j| Arc::ptr_eq(j, job)).unwrap_or(false)
    {
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
        s.tg.edit_msg(chat_id, mid, text, None).await;
    } else {
        report(s, chat_id, thread_id, pane, text).await;
    }
}

pub async fn report(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, msg: &str) {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    s.remember(chat_id, mid, pane).await;
}
