//! State-file paths (split from `state`: 300-line file limit). Pure
//! relocation — same `HERDR_STATE_DIR`-or-CWD root, same filenames.
use std::path::PathBuf;

/// Directory holding bot state files (jobs/focus/offset/topics).
/// `HERDR_STATE_DIR` overrides it; default is the launch CWD (historic
/// behavior). Launchd and manual runs MUST use the same one — split
/// directories mean replayed prompts and orphaned intents.
pub fn state_dir() -> PathBuf {
    let raw = crate::config::env_or_file("HERDR_STATE_DIR")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let Some(v) = raw else {
        return PathBuf::from(".");
    };
    // `~/` expansion parity with `HERDR_SOCKET`: a literal `~/x` dir
    // splits state from the daemon (replayed prompts, orphaned intents).
    if let Some(rest) = v.strip_prefix("~/") {
        let home = crate::types::home_dir();
        if home.is_empty() || rest.is_empty() {
            return PathBuf::from(v);
        }
        return PathBuf::from(format!("{home}/{rest}"));
    }
    PathBuf::from(v)
}

pub(crate) fn focus_file() -> PathBuf {
    state_dir().join("focus.state")
}

pub(crate) fn offset_file() -> PathBuf {
    state_dir().join("offset.state")
}

/// Back up a corrupt state file (`<path>.corrupt-<unix>.bak`, 0600):
/// single source for the offset/focus corrupt paths (dup'd backup
/// blocks re-drift — one once skipped the 0600). Log names the file
/// only (never the raw dir: it holds $HOME) plus the masked backup.
pub(crate) fn backup_corrupt(path: &std::path::Path) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bak = PathBuf::from(format!("{}.corrupt-{}.bak", path.display(), secs));
    let _ = std::fs::copy(path, &bak);
    crate::types::chmod_private(&bak);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state".to_string());
    eprintln!(
        "[main] corrupt {name} backed up to {}",
        crate::home_masked(&bak)
    );
}
