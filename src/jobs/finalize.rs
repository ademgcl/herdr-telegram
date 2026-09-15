/// Prompt result finalization: turn the live message into the final card.
/// Split from `runner` (300-line file limit). `watch_job` calls
/// `finalize` on settle; `enqueue_prompt` reports submit errors.
use std::sync::Arc;
use crate::{
    handlers::dialog::send_blocked_card,
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
    let prompt = job.prompt.lock().await.clone();
    // One settled read, arbitrated against the stream (see
    // select_final_body): alt-screen TUIs starve the delta stream, so a
    // trivial fragment must not shadow the real answer. The same screen
    // doubles as the spontaneous baseline below (no second RPC).
    let screen = read_screen(&s.cfg.socket, pane, 80).await;
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
        *job.pending.lock().await = 0;
        s.clear_pending(pane).await;
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
    *job.pending.lock().await = 0;
    s.clear_pending(pane).await;

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
    if acc_body.chars().count() >= STREAM_MIN_CHARS {
        return acc_body;
    }
    let screen_body = join_trimmed(&final_block(screen, prompt));
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
}
