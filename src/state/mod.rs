use crate::{
    config::Cfg,
    jobs::job::Job,
    jobs::persist::{self, PendingPrompt},
    telegram::TelegramClient,
    topics::TopicManager,
    types::Res,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

mod cancel;
mod jobs;

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
    /// Prompts owed a reply (pane → dest + text), mirrored to jobs.state
    /// so boot re-arms watchers orphaned by a restart.
    pub pending: Mutex<HashMap<String, PendingPrompt>>,
    pub keywait: Mutex<HashMap<(i64, Option<i64>), String>>,
    pub runwait: Mutex<HashMap<(i64, Option<i64>), String>>,
    /// Next message in this chat is typed into the pane's waiting prompt
    /// (blocked interactive input) + Enter. Set by the ⌨️ button.
    pub typewait: Mutex<HashMap<(i64, Option<i64>), String>>,
    pub nagged: Mutex<HashSet<i64>>,
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
    /// Last rate-limit episode alerted per pane: (kind, alerted_at).
    /// Watchdog-owned (prompt watchers dedupe locally): entries are
    /// cleared after consecutive confirmed-clean reads so the next episode
    /// re-alerts, and re-reminded after a long stall. Empty/outage reads
    /// preserve the entry (unknown ≠ clean) so flaps never re-alert.
    pub limit_alert: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// First sight of the current gated (`provider`/`error`) banner per
    /// pane: (kind, first_seen). Drives the stuck gate on the 60s watchdog
    /// cadence (transient blips stay silent, persistent stalls buzz once).
    /// Cleared with the episode (confirmed-clean or pane clear).
    pub limit_seen: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// Consecutive confirmed-clean (non-empty, banner-free) watchdog reads
    /// per pane. The episode clears after 2 — a single scroll/RPC flap
    /// never re-arms the alert.
    pub limit_miss: Mutex<HashMap<String, u32>>,
    /// Last displayed blocked-dialog signature per pane (question +
    /// options). Consecutive dialogs often arrive with NO status change
    /// (blocked→blocked), so content — not just transitions — decides
    /// whether a card is due. Cleared when the pane leaves blocked.
    pub blocked_sig: Mutex<HashMap<String, String>>,
    /// Panes with a button-tap in flight — observations skip posting
    /// while set (the tap owns the card update when it lands).
    pub blockop: Mutex<HashSet<String>>,
    /// Active background typing indicator tasks for working panes.
    pub typing_tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

pub type AppState = Arc<State>;

/// RAII single-flight guard: the pane is removed from the set on drop —
/// including task cancellation between insert and the manual remove —
/// so a wedged insert can never brick the pane until restart.
pub struct OpGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    pane: String,
}

impl<'a> OpGuard<'a> {
    /// Atomically claim the pane; `None` means already in flight.
    pub async fn claim(set: &'a Mutex<HashSet<String>>, pane: &str) -> Option<Self> {
        if !set.lock().await.insert(pane.to_string()) {
            return None;
        }
        Some(Self {
            set,
            pane: pane.to_string(),
        })
    }
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        // No await in Drop and never blocks the executor: single
        // try_lock, no sleep/retry (a wedged lock logs loud, never stalls
        // the tokio thread). The guard is never held across awaits, so
        // the try almost always lands.
        if let Ok(mut set) = self.set.try_lock() {
            set.remove(&self.pane);
        } else {
            eprintln!("[state] OpGuard drop wedged for {}", self.pane);
        }
    }
}

/// Directory holding bot state files (jobs/focus/offset/topics).
/// `HERDR_STATE_DIR` overrides it; default is the launch CWD (historic
/// behavior). Launchd and manual runs MUST use the same one — split
/// directories mean replayed prompts and orphaned intents.
pub fn state_dir() -> PathBuf {
    std::env::var("HERDR_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

impl State {
    fn focus_file() -> PathBuf {
        state_dir().join("focus.state")
    }

    fn offset_file() -> PathBuf {
        state_dir().join("offset.state")
    }

    /// Persist the Telegram poll offset (atomic tmp+rename, like focus):
    /// a crash between handling an update and the next poll ack must not
    /// replay the prompt and double-submit it to the agent.
    pub async fn save_offset(&self) {
        let off = *self.offset.lock().await;
        let file = Self::offset_file();
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if std::fs::write(&tmp, off.to_string()).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                eprintln!("[state] offset rename failed (disk full?)");
            }
        } else {
            eprintln!("[state] offset write failed (disk full?)");
        }
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
        // Offset survives restarts (see save_offset) — boot resumes the
        // poll stream instead of replaying the last 10 minutes of prompts.
        // Corrupt values back up like jobs.state; unparseable → 0 (the
        // router's stale filter bounds the replay).
        let offset = match std::fs::read_to_string(Self::offset_file()) {
            Err(_) => 0,
            Ok(txt) if txt.trim().is_empty() => 0,
            Ok(txt) => match txt.trim().parse::<u64>() {
                Ok(n) => n,
                Err(_) => {
                    let secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let bak = PathBuf::from(format!(
                        "{}.corrupt-{}.bak",
                        Self::offset_file().display(),
                        secs
                    ));
                    let _ = std::fs::copy(Self::offset_file(), &bak);
                    eprintln!("[main] corrupt offset.state backed up to {}", bak.display());
                    0
                }
            },
        };
        Ok(Arc::new(Self {
            cfg,
            tg,
            topics,
            offset: Mutex::new(offset),
            status: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            torder: Mutex::new(VecDeque::new()),
            focus: Mutex::new(focus),
            pending: Mutex::new(persist::load_file(&persist::store_path())),
            keywait: Mutex::new(HashMap::new()),
            runwait: Mutex::new(HashMap::new()),
            typewait: Mutex::new(HashMap::new()),
            nagged: Mutex::new(HashSet::new()),
            last_done: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            last_change: Mutex::new(HashMap::new()),
            debounce: Mutex::new(HashMap::new()),
            modelop: Mutex::new(HashSet::new()),
            limit_alert: Mutex::new(HashMap::new()),
            limit_seen: Mutex::new(HashMap::new()),
            limit_miss: Mutex::new(HashMap::new()),
            blocked_sig: Mutex::new(HashMap::new()),
            blockop: Mutex::new(HashSet::new()),
            typing_tasks: Mutex::new(HashMap::new()),
        }))
    }

    pub async fn remember(&self, chat: i64, msg_id: Option<i64>, pane: &str) {
        let Some(msg_id) = msg_id else { return };
        // Lock order (never inverted anywhere): torder → targets.
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

    /// Retire one card target (same lock order): `targets.remove` alone
    /// leaks the `torder` entry until the 512-cap overflow.
    pub async fn forget_target(&self, chat: i64, msg_id: i64) {
        let mut ord = self.torder.lock().await;
        let mut map = self.targets.lock().await;
        map.remove(&(chat, msg_id));
        ord.retain(|k| *k != (chat, msg_id));
    }

    pub async fn set_focus(&self, pane: &str) {
        let file = Self::focus_file();
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if std::fs::write(&tmp, pane).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                eprintln!("[state] focus rename failed (disk full?)");
            }
        } else {
            eprintln!("[state] focus write failed (disk full?)");
        }
        *self.focus.lock().await = Some(pane.to_string());
    }

    pub async fn get_focus(&self) -> Option<String> {
        self.focus.lock().await.clone()
    }

    /// Drop armed input waiters for a dead pane: a typewait surviving
    /// /kill would eat the owner's next message as typed input into a
    /// pane that no longer exists.
    pub async fn clear_pane(self: &Arc<Self>, pane: &str) {
        // Ownership-checked: a concurrent re-mint (same pane name reused)
        // keeps its typing task; a dead pane has no owner so it stops.
        self.stop_typing_unless_owned(pane).await;
        self.clear_waiters(pane).await;
        self.clear_targets_for(pane).await;
        // A killed pane must not stay focused: the next bare message
        // would route into the void instead of resolving fresh.
        if self.focus.lock().await.as_deref() == Some(pane) {
            *self.focus.lock().await = None;
            let _ = std::fs::remove_file(Self::focus_file());
        }
        self.status.lock().await.remove(pane);
        self.last_done.lock().await.remove(pane);
        self.seen.lock().await.remove(pane);
        self.last_change.lock().await.remove(pane);
        self.debounce.lock().await.remove(pane);
        self.clear_limit_episode(pane).await;
        self.blocked_sig.lock().await.remove(pane);
        self.modelop.lock().await.remove(pane);
        self.blockop.lock().await.remove(pane);
    }
}
