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
    /// Where the next report should be delivered (last submitter wins).
    pub dest: Mutex<(i64, Option<i64>)>,
}

impl Job {
    pub fn new(baseline: Vec<String>, chat_id: i64, thread_id: Option<i64>) -> Arc<Self> {
        Arc::new(Self {
            cancel: Notify::new(),
            stopped: AtomicBool::new(false),
            pending: Mutex::new(0),
            baseline: Mutex::new(baseline),
            dest: Mutex::new((chat_id, thread_id)),
        })
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    pub fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
