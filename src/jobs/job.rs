use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{Mutex, Notify};

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
}

impl Job {
    pub fn new(baseline: Vec<String>, chat_id: i64, thread_id: Option<i64>) -> Arc<Self> {
        let ok = !baseline.is_empty();
        Arc::new(Self {
            cancel: Notify::new(),
            stopped: AtomicBool::new(false),
            pending: Mutex::new(0),
            baseline: Mutex::new(baseline),
            baseline_ok: AtomicBool::new(ok),
            dest: Mutex::new((chat_id, thread_id)),
            prompt: Mutex::new(String::new()),
        })
    }

    pub fn baseline_ok(&self) -> bool {
        self.baseline_ok.load(Ordering::Relaxed)
    }

    /// First successful sight of the pane anchors the delta stream.
    pub async fn anchor_baseline(&self, screen: Vec<String>) {
        *self.baseline.lock().await = screen;
        self.baseline_ok.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    pub fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
