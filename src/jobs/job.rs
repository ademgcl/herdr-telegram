use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{Mutex, Notify};

use crate::types::anchorable_screen;

/// Per-pane prompt channel: submissions are delivered to the agent
/// immediately (like typing in the pane); the watcher reports output
/// whenever the agent settles.
pub struct Job {
    pub cancel: Notify,
    pub stopped: AtomicBool,
    /// Prompts submitted but not yet covered by a settle report.
    pub pending: Mutex<usize>,
    /// Pane screen at the last report — delta baseline.
    pub baseline: Mutex<Vec<String>>,
    /// False until we've seen real screen content (busy panes reject reads).
    pub baseline_ok: AtomicBool,
    /// Where the next report should be delivered (last submitter wins).
    pub dest: Mutex<(i64, Option<i64>)>,
    /// Latest prompt text (last submitter wins) — echo boundary so the
    /// fresh reply can be cut out of multi-turn scrollback.
    pub prompt: Mutex<String>,
    /// Submit epoch: bumped on every enqueue; finalize retires only if
    /// unchanged (a new prompt mid-finalize keeps the watcher alive).
    pub epoch: AtomicU64,
}

impl Job {
    pub fn new(baseline: Vec<String>, chat_id: i64, thread_id: Option<i64>) -> Arc<Self> {
        // anchor_baseline parity: an all-blank screen is never a baseline
        // (follow_answer's pre-poll read can be a cleared pane) — marking
        // it ok would finalize the whole next screen as "fresh" dupe noise.
        let ok = anchorable_screen(&baseline);
        Arc::new(Self {
            cancel: Notify::new(),
            stopped: AtomicBool::new(false),
            pending: Mutex::new(0),
            baseline: Mutex::new(baseline),
            baseline_ok: AtomicBool::new(ok),
            dest: Mutex::new((chat_id, thread_id)),
            prompt: Mutex::new(String::new()),
            epoch: AtomicU64::new(0),
        })
    }

    pub fn baseline_ok(&self) -> bool {
        self.baseline_ok.load(Ordering::Relaxed)
    }

    /// First successful sight of the pane anchors the delta stream.
    /// Never anchors an empty or all-blank screen: an outage/blank read
    /// would wipe a good baseline and repost scrollback as fresh on the
    /// next tick.
    pub async fn anchor_baseline(&self, screen: Vec<String>) {
        if !anchorable_screen(&screen) {
            return;
        }
        *self.baseline.lock().await = screen;
        self.baseline_ok.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Atomic last-wins publish (single source for enqueue/transfer):
    /// dest + prompt + generation move together under the pending lock
    /// (lock order pending→prompt→dest, never inverted anywhere — every
    /// other user takes these one at a time), so a settle snapshotting
    /// mid-submit can never pair the old epoch with the new prompt text
    /// (wrong echo boundary) or the new epoch with the old dest (stale
    /// thread delivery + leaked durable intent).
    pub async fn publish_submit(&self, chat_id: i64, thread_id: Option<i64>, text: &str) {
        let mut p = self.pending.lock().await;
        let mut prompt = self.prompt.lock().await;
        let mut dest = self.dest.lock().await;
        *dest = (chat_id, thread_id);
        *prompt = text.to_string();
        self.epoch.fetch_add(1, Ordering::Relaxed);
        *p += 1;
    }

    /// Atomic routing snapshot for settle entry (single source with
    /// `publish_submit` above — same lock order): epoch + pending +
    /// prompt read as one generation, never a split pair across a racing
    /// submit. Dest rides separately (repoint owns it mid-settle).
    pub async fn snapshot_entry(&self) -> (u64, usize, String) {
        let p = self.pending.lock().await;
        let prompt = self.prompt.lock().await;
        (self.epoch.load(Ordering::Relaxed), *p, prompt.clone())
    }

    pub fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_anchor_baseline_ignores_all_blank_screen() {
        // A cleared pane reads as non-empty all-blank lines: anchoring
        // it would make the whole next screen read as "fresh" (dupe
        // noise via the scrolled-off fallback in stream::delta).
        let job = Job::new(Vec::new(), 1, None);
        assert!(!job.baseline_ok());
        job.anchor_baseline(vec!["".to_string(), "   ".to_string()])
            .await;
        assert!(!job.baseline_ok());
        assert!((*job.baseline.lock().await).is_empty());
        // A real screen still anchors.
        job.anchor_baseline(vec!["".to_string(), "fresh".to_string()])
            .await;
        assert!(job.baseline_ok());
        assert_eq!(
            *job.baseline.lock().await,
            vec!["".to_string(), "fresh".to_string()]
        );
    }

    #[test]
    fn test_new_all_blank_baseline_starts_unanchored() {
        // Constructor parity with anchor_baseline: a cleared-pane read
        // must not start anchored, or the next screen posts whole as fresh.
        let job = Job::new(vec!["".to_string(), "   ".to_string()], 1, None);
        assert!(!job.baseline_ok());
        let job = Job::new(vec!["out".to_string()], 1, None);
        assert!(job.baseline_ok());
    }
}
