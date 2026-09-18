//! Per-pane prompt history: what the owner sent each agent (prompts,
//! typed answers, shell commands) for cross-device catch-up (`/history`).
//! Split from `state` (300-line file limit).
//!
//! RAM-only by design: prompts carry pasted secrets, so history never
//! touches disk (unlike `pending` intents) and never logs bodies.
//! Bounded per pane + pruned with the pane (see `clear_pane`).
use super::{AppState, State};
use std::collections::VecDeque;

/// Prompts remembered per pane — enough for catch-up, small enough
/// to hold in RAM (truncated at render, never at rest).
pub const HISTORY_CAP: usize = 20;

/// Bounded push shared by the recorder: oldest falls off the front.
/// Pure for tests.
pub fn record_capped(log: &mut VecDeque<String>, text: String) {
    if log.len() >= HISTORY_CAP {
        log.pop_front();
    }
    log.push_back(text);
}

/// Last `n` entries, oldest→newest, numbered. Pure for tests.
pub fn history_text(log: &VecDeque<String>, n: usize) -> String {
    let n = n.clamp(1, HISTORY_CAP);
    let take = n.min(log.len());
    log.iter()
        .skip(log.len() - take)
        .enumerate()
        .map(|(i, t)| format!("{}. {t}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

impl State {
    /// Record owner→pane text after a live deliver (never on failure:
    /// a failed submit/typed/shell send must not rewrite history).
    /// Skips blank text. Lock is short, never held across I/O.
    pub async fn push_history(&self, pane: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut map = self.history.lock().await;
        let log = map.entry(pane.to_string()).or_default();
        record_capped(log, text.to_string());
    }

    /// Last `n` owner texts for `/history`, oldest→newest. Clones
    /// under the lock, formats outside it.
    pub async fn history_render(&self, pane: &str, n: usize) -> String {
        let log = self
            .history
            .lock()
            .await
            .get(pane)
            .cloned()
            .unwrap_or_default();
        if log.is_empty() {
            return "(no history yet)".to_string();
        }
        history_text(&log, n)
    }
}

/// `/history` shared responder for every topic flavor + DM: last
/// prompts to this pane, oldest→newest. Takes the validated count
/// (callers parse via `parse_count`, already 1..=HISTORY_CAP — passing
/// raw `arg` here re-parsed leniently and diverged, e.g. `-5`
/// validated 1 but served 5). Clone under the lock, format + send
/// outside it.
pub async fn send_history(s: &AppState, chat: i64, thread: Option<i64>, pane: &str, n: usize) {
    let body = s.history_render(pane, n).await;
    s.tg.send_msg(chat, thread, &body, None).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(items: &[&str]) -> VecDeque<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_record_capped_bounds_and_orders() {
        let mut l = VecDeque::new();
        for i in 0..HISTORY_CAP + 5 {
            record_capped(&mut l, format!("m{i}"));
        }
        assert_eq!(l.len(), HISTORY_CAP);
        assert_eq!(l.front().unwrap(), "m5");
        assert_eq!(l.back().unwrap(), &format!("m{}", HISTORY_CAP + 4));
    }

    #[test]
    fn test_history_text_numbers_last_n_oldest_first() {
        let l = log(&["a", "b", "c"]);
        assert_eq!(history_text(&l, 5), "1. a\n2. b\n3. c");
        assert_eq!(history_text(&l, 2), "1. b\n2. c");
        assert_eq!(history_text(&l, 0), "1. c");
        assert_eq!(history_text(&l, 99), "1. a\n2. b\n3. c");
    }
}
