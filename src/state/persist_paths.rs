//! State-file paths (split from `state`: 300-line file limit). Pure
//! relocation — same `HERDR_STATE_DIR`-or-CWD root, same filenames.
use std::path::PathBuf;

/// Directory holding bot state files (jobs/focus/offset/topics).
/// `HERDR_STATE_DIR` overrides it; default is the launch CWD (historic
/// behavior). Launchd and manual runs MUST use the same one — split
/// directories mean replayed prompts and orphaned intents.
pub fn state_dir() -> PathBuf {
    // Test override first: a per-test dir, scoped to this thread (see
    // `set_test_dir`). Read before the env so a parallel case can never
    // observe another case's `HERDR_STATE_DIR`.
    #[cfg(test)]
    if let Some(d) = test_dir() {
        return d;
    }
    let raw = crate::config::env_or_file("HERDR_STATE_DIR")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let Some(v) = raw else {
        return PathBuf::from(".");
    };
    // `~/` expansion parity with `HERDR_SOCKET`: a literal `~/x` dir
    // splits state from the daemon (replayed prompts, orphaned intents).
    // Bare `~` or `~/` is a directory, never a state dir — fall back to
    // the launch CWD instead of minting a literal `~` folder.
    if v == "~" {
        return PathBuf::from(".");
    }
    if let Some(rest) = v.strip_prefix("~/") {
        let home = crate::types::home_dir();
        if home.is_empty() || rest.is_empty() {
            return PathBuf::from(".");
        }
        return PathBuf::from(format!("{home}/{rest}"));
    }
    PathBuf::from(v)
}

pub(crate) fn focus_file() -> PathBuf {
    state_dir().join("focus.state")
}

// Per-test state dir, thread-scoped. Cases used to get isolation from
// `set_var("HERDR_STATE_DIR")` plus a process-wide mutex held for the
// WHOLE test — which serialized every case and was UB anyway (Edition
// 2024: `set_var` races any live `env::var` elsewhere). A `#[tokio::test]`
// body runs on ONE thread, so a thread-local isolates just as well and
// lets cases run fully parallel.
#[cfg(test)]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_dir(dir: PathBuf) {
    TEST_DIR.with(|d| *d.borrow_mut() = Some(dir));
}

#[cfg(test)]
pub(crate) fn clear_test_dir() {
    TEST_DIR.with(|d| *d.borrow_mut() = None);
}

#[cfg(test)]
fn test_dir() -> Option<PathBuf> {
    TEST_DIR.with(|d| d.borrow().clone())
}

/// The same thread-local dir as $HOME for tests (hermetic against the
/// legacy `~/.local/share/herdr-telegram` migration).
#[cfg(test)]
pub(crate) fn test_home_dir() -> Option<String> {
    test_dir().map(|d| d.display().to_string())
}

pub(crate) fn offset_file() -> PathBuf {
    state_dir().join("offset.state")
}

/// Back up a corrupt state file (`<path>.corrupt-<unix>-<pid>.bak`,
/// 0600): single source for the offset/focus corrupt paths (dup'd backup
/// blocks re-drift — one once skipped the 0600). Pid-suffixed like
/// jobs/persist (secs alone collides on two corrupts in one second) and
/// pruned to 5 like every other corrupt writer (unbounded growth).
/// Log names the file only (never the raw dir: it holds $HOME) plus the
/// masked backup.
pub(crate) fn backup_corrupt(path: &std::path::Path) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bak = PathBuf::from(format!(
        "{}.corrupt-{}-{}.bak",
        path.display(),
        secs,
        std::process::id()
    ));
    let _ = std::fs::copy(path, &bak);
    crate::types::chmod_private(&bak);
    crate::types::prune_corrupt_backups(path, 5);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state".to_string());
    eprintln!(
        "[main] corrupt {name} backed up to {}",
        crate::home_masked(&bak)
    );
}
