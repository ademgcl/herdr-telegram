/// Prompt result finalization: turn the live message into the final card.
/// Split from `runner` (300-line file limit). `watch_job` calls
/// `finalize` on settle; `enqueue_prompt` reports submit errors.
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::{
    handlers::dialog::send_blocked_card,
    herdr::client::read_screen_adaptive,
    jobs::job::Job,
    jobs::segment::final_block,
    jobs::stream::join_trimmed,
    notifier::observe_status,
    state::AppState,
    types::MAX_MSG_UNITS,
    ui::chunks,
};

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
) {
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
    let screen = read_screen_adaptive(&s.cfg.socket, pane).await;
    let body = select_final_body(acc, &screen, &prompt);
    let snapshot = screen;

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
        s.seen.lock().await.insert(pane.to_string(), snapshot);
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return;
    }
    // Empty non-blocked settle: post nothing and stamp nothing, so the
    // spontaneous path re-evaluates from its own baseline.
    if body.is_empty() {
        observe_status(s, pane, settled, true, "job").await;
        settle_books(s, pane, job, entry_epoch, entry_pending).await;
        return;
    }
    let text = if settled == "blocked" {
        format!("{body}\n↩️ reply or type in topic to answer")
    } else {
        body.clone()
    };
    let parts = chunks(&text, MAX_MSG_UNITS);

    observe_status(s, pane, settled, true, "job").await;
    if settled != "blocked" {
        s.blocked_sig.lock().await.remove(pane);
    }
    // Stamp the prompt completion so the notifier can suppress the
    // redundant post-prompt idle/done echo (the card already answered),
    // and anchor the spontaneous baseline so this card is never reposted.
    s.last_done
        .lock()
        .await
        .insert(pane.to_string(), std::time::Instant::now());
    s.seen.lock().await.insert(pane.to_string(), snapshot);
    let (chat, th) = *job.dest.lock().await;
    println!("[prompt] finalize {pane}: {} part(s), body {} chars", parts.len(), body.len());
    for (i, part) in parts.iter().enumerate() {
        match (i, *live_mid) {
            (0, Some(mid)) => s.tg.edit_msg(chat, mid, part, None).await,
            _ => report(s, chat, th, pane, part).await,
        }
    }
    *live_mid = None;
    settle_books(s, pane, job, entry_epoch, entry_pending).await;
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
        s.tg.edit_msg(chat_id, mid, text, None).await;
    } else {
        report(s, chat_id, thread_id, pane, text).await;
    }
}

pub async fn report(s: &AppState, chat_id: i64, thread_id: Option<i64>, pane: &str, msg: &str) {
    let mid = s.tg.send_msg(chat_id, thread_id, msg, None).await;
    if mid.is_none() {
        eprintln!("[prompt] delivery failed {pane} (thread {thread_id:?})");
    }
    s.remember(chat_id, mid, pane).await;
}

/// Minimum streamed body trusted outright. Below this the stream is
/// assumed starved (alternate-screen TUIs serve chrome-only tails while
/// working) and the settled screen arbitrates.
const STREAM_MIN_CHARS: usize = 40;

/// Choose the final card body. The stream usually wins outright — its
/// rolling baseline (anchored at prompt time) excludes earlier turns.
/// But a starved stream must not shadow the real answer with a stray
/// line: when it yields only a fragment, the settled screen wins if it
/// is longer AND contains the fragment (reflow-proof: compared
/// whitespace-squashed, since streaming and settle reads wrap lines
/// differently). An unrelated longer screen — e.g. a coalesced
/// follow-up turn — never displaces the stream.
pub fn select_final_body(acc: &[String], screen: &[String], prompt: &str) -> String {
    let acc_body = join_trimmed(&final_block(acc, prompt));
    let screen_body = join_trimmed(&final_block(screen, prompt));
    // Fatal provider errors settle fast (often before the stream sees
    // them) while `acc` still holds the prior turn. A settled error must
    // never lose to a stale healthy stream — otherwise Telegram repeats
    // the old answer and the error vanishes.
    let screen_failed = crate::jobs::notices::screen_has_provider_failure(screen)
        || crate::jobs::notices::is_provider_failure_line(&screen_body);
    let acc_failed = crate::jobs::notices::screen_has_provider_failure(acc)
        || crate::jobs::notices::is_provider_failure_line(&acc_body);
    if screen_failed && !acc_failed && !screen_body.is_empty() {
        return screen_body;
    }
    if acc_body.chars().count() >= STREAM_MIN_CHARS {
        return acc_body;
    }
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if screen_body.len() > acc_body.len()
        && (acc_body.is_empty() || squash(&screen_body).contains(&squash(&acc_body)))
    {
        screen_body
    } else {
        acc_body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// The agy starvation shape (live wC:p2): the stream caught one
    /// wrapped line while the settled screen holds the whole answer.
    fn agy_screen() -> Vec<String> {
        v(&[
            "  • ajnow-orbit-embedding-fast:",
            "  Working tree is clean on branch",
            "  codex/embedding-fast. All 78",
            "  Jest unit and integration tests",
            "  pass cleanly.",
            "",
            "  Let me know what area you would",
            "  like to focus on next.",
            "",
            "────────────────────────────────────",
            ">",
            "────────────────────────────────────",
            "? for shortcuts             Gemini 3.8 Flash · high",
        ])
    }

    #[test]
    fn test_starved_stream_yields_to_settled_screen() {
        let acc = v(&["  pass cleanly."]);
        let body = select_final_body(&acc, &agy_screen(), "understand the project");
        assert!(body.contains("Let me know"), "full answer delivered: {body:?}");
        assert!(body.len() > 100);
    }

    #[test]
    fn test_healthy_stream_wins_despite_longer_screen() {
        // Scrollback above the echo must not displace a good stream.
        let acc = v(&["The project has three services, all green and deployed."]);
        let mut screen = v(&["older turn prose from last week that goes on a bit"]);
        screen.extend(agy_screen());
        let body = select_final_body(&acc, &screen, "status?");
        assert_eq!(body, "The project has three services, all green and deployed.");
    }

    #[test]
    fn test_short_genuine_answer_kept() {
        let acc = v(&["ok"]);
        let screen = v(&["  ┃", "  ┃  ping", "     Thought · 100ms", "     ok"]);
        assert_eq!(select_final_body(&acc, &screen, "ping"), "ok");
    }

    #[test]
    fn test_unrelated_longer_screen_never_displaces_stream() {
        let acc = v(&["first answer here"]);
        let screen = v(&["a completely different and much longer unrelated screen text"]);
        assert_eq!(select_final_body(&acc, &screen, "q"), "first answer here");
    }

    #[test]
    fn test_empty_both_ways() {
        assert_eq!(select_final_body(&[], &[], "q"), "");
    }

    #[test]
    fn test_settled_provider_error_beats_stale_stream() {
        // Prior turn streamed fine (>=40 chars) but the settled screen is
        // a fast fatal provider failure the stream never saw: the error
        // must win, never a repeat of the old answer.
        let acc = v(&["The project has three services, all green and deployed."]);
        let err = "Error from provider (Console): Upstream request failed: [invalid_request_error] reasoning `encrypted_content` was not issued to this caller";
        let screen = v(&["  ┃", &format!("  ┃  {err}"), "╹▀▀▀▀"]);
        let body = select_final_body(&acc, &screen, "do it");
        assert!(body.contains("invalid_request_error"), "error surfaced: {body:?}");
        assert!(!body.contains("three services"));
    }
}
