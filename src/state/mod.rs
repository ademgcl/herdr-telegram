use crate::{
    config::Cfg,
    jobs::job::Job,
    jobs::persist::{self, PendingPrompt},
    telegram::TelegramClient,
    topics::TopicManager,
    types::Res,
};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;

pub(crate) mod cancel;
pub(crate) mod guard;
pub(crate) mod history;
mod jobs;
mod pending_cas;
mod persist_paths;
mod targets;
mod typing;

pub use self::guard::OpGuard;
pub use self::persist_paths::state_dir;

/// Armed shell-run waiter: workspace id + arm instant (see `runwait`).
/// Alias keeps the triple-nested map under clippy's type-complexity bar.
pub(crate) type RunWait = (String, std::time::Instant);
/// Armed input waiter: target pane + arm instant (keywait, typewait).
/// Age expiry lives in hygiene (values are panes, so pane-liveness
/// reap applies too — unlike runwait's workspace ids).
pub(crate) type Waiter = (String, std::time::Instant);

/// Global async-lock order (never inverted anywhere, never held across
/// sleep/RPC — all critical sections snapshot-then-drop):
/// `status` → `last_change`, `torder` → `targets`, `pending` → `shell_gen`.
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
    /// Next message in this chat is sent as raw keys to the pane (set by
    /// the K button). (Pane, armed_at): age expiry in hygiene — stale
    /// keys executing into a live session is a write that must not fire
    /// arbitrarily later.
    pub keywait: Mutex<HashMap<(i64, Option<i64>), Waiter>>,
    /// Next message in this chat runs as a shell command in the armed
    /// workspace (set by the R button). (Workspace, armed_at): values
    /// are workspace ids, never panes — hygiene must expire by AGE, not
    /// by pane-liveness (see reap), or an armed waiter fires arbitrarily
    /// later as a shell command.
    pub runwait: Mutex<HashMap<(i64, Option<i64>), RunWait>>,
    /// Next message in this chat is typed into the pane's waiting prompt
    /// (blocked interactive input) + Enter. Set by the keyboard button.
    /// (Pane, armed_at): age expiry in hygiene (generous — answers take
    /// a while to compose; expiry degrades to normal routing).
    pub typewait: Mutex<HashMap<(i64, Option<i64>), Waiter>>,
    /// Setup-note stamp per chat (see NAGGED_SECS): time-bounded like
    /// stale notices — an unconfigured group reminds daily, never
    /// forever-mute, never spam.
    pub nagged: Mutex<HashMap<i64, std::time::Instant>>,
    /// Last stale-arrival notice per (chat, thread): a boot burst queues
    /// N stale messages and each must not send its own "please resend".
    /// Time-bounded (STALE_SECS), never forever — a later genuine stall
    /// in the same thread still notifies.
    pub stale_nagged: Mutex<HashMap<(i64, Option<i64>), std::time::Instant>>,
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
    pub modelop: Mutex<HashMap<String, std::time::Instant>>,
    /// Last rate-limit episode alert per pane: (kind, at). Shared by
    /// watchdog + watchers (atomic claim ⇒ handoffs page once); cleared
    /// after confirmed-clean reads, re-reminded while stalls persist.
    /// Empty reads preserve it, failed sends remove it (see limits.rs).
    pub limit_alert: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// First sight of the current banner per pane: (kind, first_seen).
    /// Drives the stuck gate (gated kinds) and flip damping (immediate
    /// kinds) on the 60s watchdog cadence; accumulates across settled
    /// samples too, so a return to working fires promptly. Cleared with
    /// the episode (confirmed-clean or pane clear).
    pub limit_seen: Mutex<HashMap<String, (String, std::time::Instant)>>,
    /// Consecutive confirmed-clean (non-empty, banner-free) watchdog reads
    /// per pane. The episode clears after 3 — a single scroll/RPC flap
    /// never re-arms the alert.
    pub limit_miss: Mutex<HashMap<String, u32>>,
    /// Last failed limit-alert send per pane (shared send-failure backoff).
    pub limit_send_cool: Mutex<HashMap<String, std::time::Instant>>,
    /// Last displayed blocked-dialog signature per pane (question +
    /// options). Consecutive dialogs often arrive with NO status change
    /// (blocked→blocked), so content — not just transitions — decides
    /// whether a card is due. Cleared when the pane leaves blocked.
    pub blocked_sig: Mutex<HashMap<String, String>>,
    /// Panes with a button-tap in flight — observations skip posting
    /// while set (the tap owns the card update when it lands).
    pub blockop: Mutex<HashMap<String, std::time::Instant>>,
    /// Spawn taps in flight (`spawn:<chat>:<msg>`): never live panes, so
    /// never in `blockop` (hygiene would reap them as dead panes and a
    /// double-tap would mint two spaces + agents). Age-only expiry + prune
    /// in hygiene, same bound as taps.
    pub spawnop: Mutex<HashMap<String, std::time::Instant>>,
    /// Completed spawn taps (`spawn:<chat>:<msg>` → done instant): the
    /// pump runs queued taps sequentially, so a transient in-flight guard
    /// alone still double-mints. Retaps within the window stand down;
    /// retry-after-failure uses a fresh card (fresh key). Age-only expiry
    /// + prune in hygiene, same bound.
    pub spawndone: Mutex<HashMap<String, std::time::Instant>>,
    /// Posted blocked-card locations per pane: (chat, msg) — one per
    /// chat (forum once, each owner DM once). A PC-side answer strips
    /// these (buttons must not outlive the dialog); resolve consumes.
    pub blocked_card: Mutex<HashMap<String, Vec<(i64, i64)>>>,
    /// Owner→pane texts for `/history` catch-up (RAM-only: prompts
    /// carry secrets, never disk). Bounded per pane, pruned with it.
    pub history: Mutex<HashMap<String, std::collections::VecDeque<String>>>,
    /// Shell submit generation per pane (identical re-commands share one
    /// slot — text equality alone can't tell settles apart). Pruned live.
    pub shell_gen: Mutex<HashMap<String, u64>>,
    /// Active background typing indicator tasks for working panes.
    pub typing_tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

pub type AppState = Arc<State>;

/// Typing-indicator cadence: Telegram clients expire `typing` after ~5s
/// with no resend, and a returning client shows nothing until the next
/// action lands — so every sustainer re-fires well inside the window
/// (worst passive-return gap ≈ this). Single source for the typing task,
/// the watcher piggyback, and the submit sustain loops.
pub(crate) const TYPING_TICK_SECS: u64 = 2;

impl State {
    /// Persist the Telegram poll offset (atomic tmp+rename, like focus):
    /// a crash between handling an update and the next poll ack must not
    /// replay the prompt and double-submit it to the agent.
    pub async fn save_offset(&self) {
        let off = *self.offset.lock().await;
        let file = persist_paths::offset_file();
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if crate::types::write_private(&tmp, off.to_string().as_bytes()).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                eprintln!("[state] offset rename failed (disk full?)");
            }
        } else {
            eprintln!("[state] offset write failed (disk full?)");
        }
    }

    pub fn new(cfg: Cfg) -> Res<AppState> {
        let tg = TelegramClient::new(cfg.token.clone())?;
        let topics = TopicManager::new(cfg.forum, tg.clone(), Some(cfg.socket.clone()));
        // Survive restarts: routing target must not vanish on redeploy
        let file = persist_paths::focus_file();
        let home = crate::types::home_dir();
        let legacy = PathBuf::from(format!("{home}/.local/share/herdr-telegram/focus"));
        if !file.exists() && legacy.exists() {
            let _ = std::fs::copy(&legacy, &file);
            crate::types::chmod_private(&file);
        }
        let focus = match std::fs::read_to_string(&file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(f) if crate::types::valid_focus(&f) => Some(f),
            // Torn write / hand-edit garbage must never become the DM
            // routing focus (bare messages would route into a dead pane).
            // Back up like offset.state, then fall back to unfocused.
            Some(_) => {
                persist_paths::backup_corrupt(&file);
                None
            }
            None => None,
        };
        // Offset survives restarts (see save_offset) — boot resumes the
        // poll stream instead of replaying the last 10 minutes of prompts.
        // Corrupt values back up like jobs.state; unparseable → 0 (the
        // router's stale filter bounds the replay).
        let offset = match std::fs::read_to_string(persist_paths::offset_file()) {
            Err(_) => 0,
            Ok(txt) if txt.trim().is_empty() => 0,
            Ok(txt) => match txt.trim().parse::<u64>() {
                Ok(n) => n,
                Err(_) => {
                    persist_paths::backup_corrupt(&persist_paths::offset_file());
                    0
                }
            },
        };
        let st = Arc::new(Self {
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
            nagged: Mutex::new(HashMap::new()),
            stale_nagged: Mutex::new(HashMap::new()),
            last_done: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            last_change: Mutex::new(HashMap::new()),
            debounce: Mutex::new(HashMap::new()),
            modelop: Mutex::new(HashMap::new()),
            limit_alert: Mutex::new(HashMap::new()),
            limit_seen: Mutex::new(HashMap::new()),
            limit_miss: Mutex::new(HashMap::new()),
            limit_send_cool: Mutex::new(HashMap::new()),
            blocked_sig: Mutex::new(HashMap::new()),
            blockop: Mutex::new(HashMap::new()),
            spawnop: Mutex::new(HashMap::new()),
            spawndone: Mutex::new(crate::handlers::callback_spawn::load_spawndone()),
            blocked_card: Mutex::new(HashMap::new()),
            history: Mutex::new(HashMap::new()),
            shell_gen: Mutex::new(HashMap::new()),
            typing_tasks: Mutex::new(HashMap::new()),
        });
        // Orphan-heal: `last_msgs` entries without a mapping are
        // write-only disk growth (stale mids also resurface via
        // `reset_topic` copy_msg into reminted topics). Prune at boot in
        // every mode — DM→forum switches orphan the same way.
        st.topics.storage.prune_orphan_msgs();
        Ok(st)
    }

    /// Drop armed input waiters for a dead pane: a typewait surviving
    /// /kill would eat the owner's next message as typed input into a
    /// pane that no longer exists.
    pub async fn clear_pane(self: &Arc<Self>, pane: &str) {
        // Ownership-checked: a concurrent re-mint (same pane name reused)
        // keeps its typing task; a dead pane has no owner so it stops.
        self.stop_typing_unless_owned(pane).await;
        self.clear_waiters(pane).await;
        // Reply targets are NEVER pruned here: a DM reply to a corpse
        // card must fail visibly ("pane gone") via the shell fallback —
        // pruning it would silently reroute the text as a prompt into
        // the focused live agent. Dead entries age out via the 512-cap
        // overflow in `remember`; per-tap `forget_target` drops them.
        // A killed pane must not stay focused: the next bare message
        // would route into the void instead of resolving fresh. Single
        // lock check-and-clear: a set_focus landing between a split check
        // and clear would else wipe the FRESH focus (next bare message
        // loses its target — sole-agent fallback or error).
        {
            let mut focus = self.focus.lock().await;
            if focus.as_deref() == Some(pane) {
                *focus = None;
                let _ = std::fs::remove_file(persist_paths::focus_file());
            }
        }
        self.status.lock().await.remove(pane);
        self.last_done.lock().await.remove(pane);
        self.seen.lock().await.remove(pane);
        self.last_change.lock().await.remove(pane);
        self.debounce.lock().await.remove(pane);
        self.clear_limit_episode(pane).await;
        self.blocked_sig.lock().await.remove(pane);
        self.blocked_card.lock().await.remove(pane);
        self.history.lock().await.remove(pane);
        // Fresh guards die with the pane (remints must not inherit a
        // live-tap brick or a stale shell generation): a flapped list
        // sample races like jobs/waiters (pre-existing, shared) and the
        // next tap claims fresh.
        self.modelop.lock().await.remove(pane);
        self.blockop.lock().await.remove(pane);
        self.shell_gen.lock().await.remove(pane);
    }
}
