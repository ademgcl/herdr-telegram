use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;
use crate::{
    config::Cfg,
    jobs::job::Job,
    telegram::client::TelegramClient,
    topics::TopicManager,
    types::Res,
};

pub struct State {
    pub cfg: Cfg,
    pub tg: TelegramClient,
    pub topics: TopicManager,
    pub offset: Mutex<u64>,
    pub status: Mutex<HashMap<String, String>>,
    pub jobs: Mutex<HashMap<String, Arc<Job>>>,
    pub targets: Mutex<HashMap<(i64, i64), String>>,
    pub torder: Mutex<VecDeque<(i64, i64)>>,
    pub focus: Mutex<Option<String>>,
    pub keywait: Mutex<HashMap<i64, String>>,
    pub runwait: Mutex<HashMap<i64, String>>,
    /// Next message in this chat is typed into the pane's waiting prompt
    /// (blocked interactive input) + Enter. Set by the ⌨️ button.
    pub typewait: Mutex<HashMap<i64, String>>,
    /// When a prompt watcher last reported a pane — status alerts inside
    /// this window are redundant (the final card already covered them).
    pub last_done: Mutex<HashMap<String, std::time::Instant>>,
    /// Last screen snapshot per pane — baseline for spontaneous output:
    /// settles with no fresh delta repost nothing (idempotent).
    pub seen: Mutex<HashMap<String, Vec<String>>>,
    /// When each pane's status last changed — done↔idle flap is only
    /// collapsed when genuinely rapid (push events), never for slow
    /// watchdog-sampled settles which are legitimate completions.
    pub last_change: Mutex<HashMap<String, std::time::Instant>>,
    /// Armed settle debounce per pane: (status, armed_at). A newer settle
    /// supersedes; the task posts only if still current when it fires.
    pub debounce: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// Panes with a model switch in flight — second taps wait.
    pub modelop: Mutex<HashSet<String>>,
    /// When each pane last entered a settled state — drives the
    /// done→idle display decay (herdr parks agents at done indefinitely).
    pub settled_at: Mutex<HashMap<String, std::time::Instant>>,
    /// Last rate-limit episode alerted per pane: (kind, alerted_at).
    /// Watchdog-owned (prompt watchers dedupe locally): entries are
    /// cleared when the pattern leaves the screen so the next episode
    /// re-alerts, and re-reminded after a long stall.
    pub limit_alert: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// Last displayed blocked-dialog signature per pane (question +
    /// options). Consecutive dialogs often arrive with NO status change
    /// (blocked→blocked), so content — not just transitions — decides
    /// whether a card is due. Cleared when the pane leaves blocked.
    pub blocked_sig: Mutex<HashMap<String, String>>,
    /// Panes with a button-tap in flight — observations skip posting
    /// while set (the tap owns the card update when it lands).
    pub blockop: Mutex<HashSet<String>>,
}

pub type AppState = Arc<State>;

impl State {
    fn focus_file() -> PathBuf {
        PathBuf::from("focus.state")
    }

    pub fn new(cfg: Cfg) -> Res<AppState> {
        let tg = TelegramClient::new(cfg.token.clone())?;
        let topics = TopicManager::new(cfg.forum, tg.clone());
        // Survive restarts: routing target must not vanish on redeploy
        let file = Self::focus_file();
        let home = std::env::var("HOME").unwrap_or_default();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/focus"));
        if !file.exists() && legacy.exists() {
            let _ = std::fs::copy(&legacy, &file);
        }
        let focus = std::fs::read_to_string(&file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(Arc::new(Self {
            cfg,
            tg,
            topics,
            offset: Mutex::new(0),
            status: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            torder: Mutex::new(VecDeque::new()),
            focus: Mutex::new(focus),
            keywait: Mutex::new(HashMap::new()),
            runwait: Mutex::new(HashMap::new()),
            typewait: Mutex::new(HashMap::new()),
            last_done: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            last_change: Mutex::new(HashMap::new()),
            debounce: Mutex::new(HashMap::new()),
            modelop: Mutex::new(HashSet::new()),
            settled_at: Mutex::new(HashMap::new()),
            limit_alert: Mutex::new(HashMap::new()),
            blocked_sig: Mutex::new(HashMap::new()),
            blockop: Mutex::new(HashSet::new()),
        }))
    }

    pub async fn remember(&self, chat: i64, msg_id: Option<i64>, pane: &str) {
        let Some(msg_id) = msg_id else { return };
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        while map.len() >= 512 {
            match ord.pop_front() {
                Some(old) => {
                    map.remove(&old);
                }
                None => break,
            }
        }
        if map.insert((chat, msg_id), pane.to_string()).is_none() {
            ord.push_back((chat, msg_id));
        }
    }

    pub async fn set_focus(&self, pane: &str) {
        let _ = std::fs::write(Self::focus_file(), pane);
        *self.focus.lock().await = Some(pane.to_string());
    }

    pub async fn get_focus(&self) -> Option<String> {
        self.focus.lock().await.clone()
    }

    /// Retire one pane's watcher (used by /quit: no agent left to watch).
    pub async fn cancel_jobs_for(&self, pane: &str) -> bool {
        let job = self.jobs.lock().await.remove(pane);
        match job {
            Some(job) => {
                job.mark_stopped();
                job.cancel.notify_waiters();
                true
            }
            None => false,
        }
    }

    pub async fn cancel_all_jobs(&self) -> usize {        let jobs: HashMap<String, Arc<Job>> = std::mem::take(&mut *self.jobs.lock().await);
        let count = jobs.len();
        for job in jobs.values() {
            job.mark_stopped();
            job.cancel.notify_waiters();
        }
        count
    }
}
