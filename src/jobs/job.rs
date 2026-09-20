use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
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
    /// Submit epoch: bumped on every enqueue; finalize retires only if
    /// unchanged (a new prompt mid-finalize keeps the watcher alive).
    pub epoch: AtomicU64,
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
            epoch: AtomicU64::new(0),
        })
    }

    pub fn baseline_ok(&self) -> bool {
        self.baseline_ok.load(Ordering::Relaxed)
    }

    /// First successful sight of the pane anchors the delta stream.
    /// Never anchors an empty screen: an outage/blank read would wipe a
    /// good baseline and repost scrollback as fresh on the next tick.
    pub async fn anchor_baseline(&self, screen: Vec<String>) {
        if screen.is_empty() {
            return;
        }
        *self.baseline.lock().await = screen;
        self.baseline_ok.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Bump epoch + pending atomically under the pending lock (no await
    /// inside): finalize snapshots both under the same lock, so the
    /// entry share never skews across a racing submit. Single source
    /// with `snapshot_generation` below.
    pub async fn bump_generation(&self) {
        let mut p = self.pending.lock().await;
        self.epoch.fetch_add(1, Ordering::Relaxed);
        *p += 1;
    }

    /// Snapshot epoch + pending under the pending lock (no await
    /// inside): pairs with `bump_generation` above — a submit landing
    /// between two detached reads would else over/under-count the
    /// entry share into the newcomer's cover.
    pub async fn snapshot_generation(&self) -> (u64, usize) {
        let p = self.pending.lock().await;
        (self.epoch.load(Ordering::Relaxed), *p)
    }

    pub fn mark_stopped(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}
