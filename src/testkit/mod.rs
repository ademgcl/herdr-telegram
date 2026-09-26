//! E2E harness: a real `AppState` wired to a fake Telegram API and a
//! fake herdr socket, driven through the production update router. No
//! internal calls are stubbed — the point is to see exactly what the
//! owner would see.

use crate::state::AppState;
use serde_json::{Value, json};
use std::sync::Arc;

pub mod herdr_fake;
pub mod tg_fake;

pub use herdr_fake::FakeHerdr;
pub use tg_fake::FakeTg;

#[cfg(test)]
#[path = "e2e_delivery_tests.rs"]
mod e2e_delivery;
#[cfg(test)]
#[path = "e2e_handler_tests.rs"]
mod e2e_handler;
#[cfg(test)]
#[path = "e2e_state_tests.rs"]
mod e2e_state;

pub const OWNER: i64 = 7;

/// Live harness: state + both fakes + the fake socket path.
pub struct Harness {
    pub s: AppState,
    pub tg: Arc<FakeTg>,
    pub herdr: Arc<FakeHerdr>,
    /// This case's identity: unique chat/thread/pane so parallel cases
    /// never see each other's traffic in the shared fake.
    pub chat: i64,
    pub thread: i64,
    pub pane: String,
    /// Keeps the isolated state dir (and its env guard) alive.
    _dir: crate::state::cancel::TestStateDir,
}

impl Harness {
    /// Fakes first, then ONE state mint pointed at the fake socket —
    /// two mints would deadlock on the env guard `isolated_state` holds.
    /// The env var is set BEFORE the mint: `TelegramClient::new` reads
    /// it there (test builds only), and the guard already serializes
    /// env access against every other test.
    pub async fn start() -> Harness {
        // One fake per binary: the env override is written once (the
        // URL never changes), so parallel cases cannot steal it.
        let (tg, url) = tg_fake::start();
        let n = case_seq();
        let chat = 5_000 + n;
        let thread = 4_000 + n;
        let pane = format!("w{}:p1", n + 1);
        let dir = std::env::temp_dir().join(socket_seq());
        std::fs::create_dir_all(&dir).expect("herdr fake dir");
        let (herdr, sock) = herdr_fake::start(&dir).await;
        unsafe { std::env::set_var("HERDR_TG_FAKE_BASE", url) };
        let (s, dir_guard) =
            crate::state::cancel::isolated_state_for(&sock, Some(chat), vec![OWNER]);
        Harness {
            s,
            tg,
            herdr,
            chat,
            thread,
            pane,
            _dir: dir_guard,
        }
    }

    // ---- chat-scoped assertions (this case's traffic only) ----
    pub fn calls(&self, method: &str) -> Vec<tg_fake::Call> {
        self.tg.calls_of(self.chat, method)
    }
    pub fn sends(&self) -> Vec<tg_fake::Call> {
        self.calls("sendMessage")
    }
    pub fn edits(&self) -> Vec<tg_fake::Call> {
        self.calls("editMessageText")
    }
    pub fn sent_texts(&self) -> Vec<String> {
        self.tg.sent_texts(self.chat)
    }
    pub fn sent_count(&self, method: &str) -> usize {
        self.tg.sent_count(self.chat, method)
    }
    pub fn fault(&self, method: &str, description: &str) {
        self.tg.fault_next(self.chat, method, description);
    }

    /// Map this case's pane to its topic thread (real forum mapping) so
    /// topic routing and typing work exactly as in production.
    pub fn map_topic(&self) {
        self.s.topics.storage.insert(self.pane.clone(), self.thread);
        self.s.topics.storage.set_title(&self.pane, "e2e");
    }

    /// Deliver one owner message through the real router. The router
    /// requires `chat.type` and a fresh `date` (a dateless update is
    /// dropped as ambiguous), so the harness supplies both.
    pub async fn say(&self, text: &str) {
        self.update(self.message(json!({"text": text}))).await;
    }

    /// Deliver a photo with a caption through the real router.
    pub async fn say_photo(&self, caption: &str, file_id: &str) {
        self.update(self.message(json!({
            "caption": caption,
            "photo": [{"file_id": file_id, "width": 100, "height": 100,
                        "file_size": 10}],
        })))
        .await;
    }

    /// Message skeleton: owner in the forum topic, stamped fresh.
    pub fn message(&self, body: Value) -> Value {
        let mut m = body;
        let obj = m.as_object_mut().expect("message body object");
        obj.insert("message_id".into(), json!(next_msg_id()));
        obj.insert("from".into(), json!({"id": OWNER}));
        obj.insert(
            "chat".into(),
            json!({"id": self.chat, "type": "supergroup", "title": "e2e"}),
        );
        if !obj.contains_key("message_thread_id") {
            obj.insert("message_thread_id".into(), json!(self.thread));
        }
        obj.insert("date".into(), json!(now_unix()));
        m
    }

    /// Tap callback (answer buttons, keys, cards).
    #[allow(dead_code)] // used by the blocked-card cases when added
    pub async fn tap(&self, data: &str) {
        self.update(json!({
            "callback_query": {
                "id": "cb1",
                "from": {"id": OWNER},
                "chat_instance": "x",
                "message": {"message_id": 1,
                            "chat": {"id": self.chat, "type": "supergroup"},
                            "message_thread_id": self.thread,
                            "date": now_unix()},
                "data": data,
            }
        }))
        .await;
    }

    pub async fn update(&self, message: Value) {
        let update = json!({"update_id": next_update_id(), "message": message});
        let s = self.s.clone();
        crate::telegram::router::handle_update(s, &update).await;
    }

    /// Wait until `cond` holds (or fail with a readable dump).
    pub async fn wait_for(&self, what: &str, cond: impl Fn() -> bool) {
        self.wait_for_async(what, || async { cond() }).await;
    }

    /// Async-cond variant (state behind a lock needs an await inside).
    pub async fn wait_for_async<F, Fut>(&self, what: &str, mut cond: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..120 {
            if cond().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!(
            "timed out waiting for {what}\n--- telegram calls (chat {}) ---\n{:#?}",
            self.chat,
            self.calls("sendMessage")
        );
    }

    /// Typing tasks currently held for a pane (0 = no stuck indicator).
    pub async fn typing_tasks(&self) -> usize {
        self.s.typing_tasks.lock().await.len()
    }

    /// Wait until a NEW final lands. Skips the instant placeholder by
    /// content (it is the turn's first send) and counts from `seen`, so
    /// neither the previous turn's final nor this turn's working
    /// message can satisfy the wait.
    pub async fn wait_for_final_after(&self, seen: usize) -> Vec<tg_fake::Call> {
        self.wait_for("final card", || {
            self.sends()
                .iter()
                .skip(seen)
                .any(|c| !c.text().contains(crate::jobs::progress::THINKING))
        })
        .await;
        self.sends()
    }

    /// Let the watcher run a few poll cycles.
    pub async fn tick(&self, n: u32) {
        for _ in 0..n {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        }
    }

    /// Full happy-path drive: prompt in, agent works, agent answers.
    /// `reply` lines land on the pane as the agent's output (with
    /// realistic chrome around them — a bare "thinking" word is
    /// PROSE and must survive filtering, so the fake must not fake it).
    pub async fn run_turn(&self, prompt: &str, reply: &[&str]) {
        let seen = self.sent_count("sendMessage");
        let submits = self.herdr.submit_count();
        self.herdr.set_status("working");
        self.herdr
            .set_screen(&[&format!("> {prompt}"), "Thinking…"]);
        self.say(prompt).await;
        self.wait_for("submit", || self.herdr.submit_count() > submits)
            .await;
        let mut screen: Vec<String> = vec![format!("> {prompt}"), "Thinking…".into()];
        screen.extend(reply.iter().map(|l| l.to_string()));
        screen.push(" ⬝ esc interrupt   145.6K (14%)  ctrl+p commands".into());
        self.herdr.set_screen_owned(screen);
        self.herdr.set_status("idle");
        self.wait_for_final_after(seen).await;
    }
}

fn next_update_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static N: AtomicI64 = AtomicI64::new(1000);
    N.fetch_add(1, Ordering::Relaxed)
}

fn next_msg_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static N: AtomicI64 = AtomicI64::new(500);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Fresh-enough unix seconds (the router drops dateless/stale updates).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Unique per-case identity (chat/thread/pane numbering).
fn case_seq() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static N: AtomicI64 = AtomicI64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Unique per-harness fake-socket dir (parallel tests, one process).
fn socket_seq() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "herdr-fake-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}
