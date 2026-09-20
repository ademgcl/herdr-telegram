//! Answer-follow watcher: after a blocked answer (tap/type) resumes
//! the agent, track the resumed turn to its final reply. Split from
//! `runner`/`enqueue` (300-line file limit).
//!
//! Why: taps/types own no job — the final reply relied solely on the
//! spontaneous 15s debounce, which aborts when a new prompt lands first
//! (`jobs.contains` guard). The new prompt's watcher then baselines past
//! the answer-turn's output, so that reply never lands. A dedicated
//! follower finalizes in ~5s like prompt jobs, beating the race.
//!
//! In-memory only (no durable intent): the turn is seconds-long; a
//! restart mid-turn loses it like spontaneous already did. Skipping the
//! disk write also avoids last-wins races with a concurrent new prompt.
use crate::{
    herdr::client::{get_agent, read_screen},
    jobs::job::Job,
    state::AppState,
};

/// Pure stand-down rule (tested): a live (non-stopped) job owns the pane,
/// or the agent never resumed (still `blocked`, or the status read failed
/// and `status` is empty) — the card path owns those, the follower stands
/// down. Only a resumed, watcherless turn starts a follower.
pub fn should_start_follow(has_live_job: bool, status: &str) -> bool {
    !has_live_job && !status.is_empty() && status != "blocked"
}

/// Track the resumed turn after a successful blocked answer.
/// `hint` is the answer label/text (echo boundary only, never posted).
/// Stands down when a live job owns the pane or the pane is still
/// blocked/unreadable (the card path owns those).
pub async fn follow_answer(s: &AppState, pane: &str, chat: i64, thread: Option<i64>, hint: &str) {
    // A tap/type answer resumes with status lag: herdr still samples
    // `blocked` for seconds after the resume, and RPC blips read the
    // same. Poll up to ~6s instead of abandoning the resumed turn
    // watcherless after one beat (the race this follower exists for:
    // taps classified Unchanged on lagging status never even called us).
    for _ in 0..6 {
        if s.jobs
            .lock()
            .await
            .get(pane)
            .is_some_and(|j| !j.is_stopped())
        {
            return;
        }
        match get_agent(&s.cfg.socket, pane).await {
            Ok(a) if should_start_follow(false, &a.status) => break,
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if s.jobs
        .lock()
        .await
        .get(pane)
        .is_some_and(|j| !j.is_stopped())
    {
        return;
    }
    match get_agent(&s.cfg.socket, pane).await {
        Ok(a) if should_start_follow(false, &a.status) => {}
        _ => return,
    }
    let baseline = read_screen(&s.cfg.socket, pane, 400).await;
    let job = Job::new(baseline, chat, thread);
    *job.prompt.lock().await = hint.to_string();
    *job.pending.lock().await = 1;
    {
        // Single source for the atomic claim (enqueue/recover parity):
        // a live watcher winning the poll gap stands — never two
        // watchers on one pane.
        let mut map = s.jobs.lock().await;
        if !super::recover::claim_watcher(&mut map, pane, job.clone()) {
            return;
        }
    }
    println!("[jobs] follow-answer watcher for {pane}");
    tokio::spawn(super::runner::watch_job(s.clone(), pane.to_string(), job));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_follow_stands_down_on_live_job() {
        assert!(!should_start_follow(true, "working"));
        assert!(!should_start_follow(true, "idle"));
    }

    #[test]
    fn test_follow_stands_down_when_not_resumed() {
        // Still blocked: the card path owns it.
        assert!(!should_start_follow(false, "blocked"));
        // Unreadable (RPC failed): the card path owns it too.
        assert!(!should_start_follow(false, ""));
    }

    #[test]
    fn test_follow_starts_on_resumed_watcherless_turn() {
        assert!(should_start_follow(false, "working"));
        assert!(should_start_follow(false, "idle"));
        assert!(should_start_follow(false, "done"));
    }
}
