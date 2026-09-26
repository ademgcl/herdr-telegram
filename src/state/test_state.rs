//! Isolated AppState for tests (shared by cancel/hygiene/ctl suites).
/// Cancel + reap paths persist jobs.state, so tests must never touch the
/// repo's live files: each call mints a fresh temp state dir. Serialized
/// via a static mutex: `set_var`/`var` is UB under parallel `cargo test`
/// (Edition 2024 marks it unsafe), so holders keep the guard for the
/// whole test (`_dir` alive) and restore the prior value on drop.
/// Split from `cancel` (300-line file limit); re-exported there so
/// existing `state::cancel::isolated_state` call sites keep working.
use super::State;

#[cfg(test)]
static TEST_ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct TestStateDir {
    path: std::path::PathBuf,
    _guard: std::sync::MutexGuard<'static, ()>,
    old: Option<std::ffi::OsString>,
    old_home: Option<std::ffi::OsString>,
}
#[cfg(test)]
impl Drop for TestStateDir {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            unsafe { std::env::set_var("HERDR_STATE_DIR", old) };
        } else {
            unsafe { std::env::remove_var("HERDR_STATE_DIR") };
        }
        if let Some(old) = self.old_home.take() {
            unsafe { std::env::set_var("HOME", old) };
        } else {
            unsafe { std::env::remove_var("HOME") };
        }
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
    let guard = TEST_ENV_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::var_os("HERDR_STATE_DIR");
    let old_home = std::env::var_os("HOME");
    let n = TEST_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("ht-{n}-{nanos}"));
    std::fs::create_dir_all(&path).expect("test tempdir");
    unsafe { std::env::set_var("HERDR_STATE_DIR", &path) };
    // Hermetic HOME: State::new + TopicStorage migrate legacy
    // `~/.local/share/herdr-telegram/{focus,topics.json}` when the fresh
    // dir is empty. Without this the dev machine's real focus (e.g.
    // `w8:p1`) leaks into every isolated state and stale focus shadows
    // the sole-agent fallback (see handlers::target dm_pane).
    unsafe { std::env::set_var("HOME", &path) };
    let cfg = crate::config::Cfg {
        token: "test-token".to_string(),
        socket: "nonexistent-test.sock".to_string(),
        owners: vec![],
        forum,
    };
    let s = State::new(cfg).expect("test state");
    (
        s,
        TestStateDir {
            path,
            _guard: guard,
            old,
            old_home,
        },
    )
}
