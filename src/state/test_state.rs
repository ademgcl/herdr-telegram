//! Isolated AppState for tests (shared by cancel/hygiene/ctl suites).
//! Cancel + reap paths persist jobs.state, so tests must never touch the
//! repo's live files: each call mints a fresh temp state dir, published
//! as a THREAD-LOCAL (see `persist_paths::set_test_dir`) rather than an
//! env var. Every test is a `#[tokio::test]`, so its body — and every
//! task it spawns — stays on that one thread and the dir is exactly as
//! isolated as before. The env var is the wrong tool: `set_var` is UB
//! beside a live `env::var` on another thread (Edition 2024), and the
//! process-wide mutex that papered over it was held across the whole
//! test, serializing every case and leaving 7 of 8 cores idle.
//! Split from `cancel` (500-line file limit); re-exported there so
//! existing `state::cancel::isolated_state` call sites keep working.
use super::State;

#[cfg(test)]
pub(crate) struct TestStateDir {
    path: std::path::PathBuf,
}
#[cfg(test)]
impl Drop for TestStateDir {
    fn drop(&mut self) {
        super::persist_paths::clear_test_dir();
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
static TEST_DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn isolated_state() -> (super::AppState, TestStateDir) {
    isolated_state_with_forum(None)
}

/// Forum-mode isolated state: typing tasks need `cfg.forum` (they
/// no-op in DM), so cancel/typing ownership tests mint with a chat id.
#[cfg(test)]
pub(crate) fn isolated_state_with_forum(forum: Option<i64>) -> (super::AppState, TestStateDir) {
    isolated_state_for("nonexistent-test.sock", forum, vec![])
}

/// E2E harness state: like `isolated_state_with_forum` but pointed at
/// the fake herdr socket with real owners, so prompts route and submit.
#[cfg(test)]
pub(crate) fn isolated_state_for(
    socket: &str,
    forum: Option<i64>,
    owners: Vec<i64>,
) -> (super::AppState, TestStateDir) {
    let n = TEST_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("ht-{n}-{nanos}"));
    std::fs::create_dir_all(&path).expect("test tempdir");
    // Hermetic HOME: State::new + TopicStorage migrate legacy
    // `~/.local/share/herdr-telegram/{focus,topics.json}` when the fresh
    // dir is empty. Without this the dev machine's real focus (e.g.
    // `w8:p1`) leaks into every isolated state and stale focus shadows
    // the sole-agent fallback (see handlers::target dm_pane). The
    // thread-local dir doubles as HOME — same value the env var carried.
    super::persist_paths::set_test_dir(path.clone());
    let cfg = crate::config::Cfg {
        token: "test-token".to_string(),
        socket: socket.to_string(),
        owners,
        forum,
    };
    let s = State::new(cfg).expect("test state");
    (s, TestStateDir { path })
}
