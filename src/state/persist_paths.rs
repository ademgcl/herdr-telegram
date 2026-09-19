//! State-file paths (split from `state`: 300-line file limit). Pure
//! relocation — same `HERDR_STATE_DIR`-or-CWD root, same filenames.
use std::path::PathBuf;

/// Directory holding bot state files (jobs/focus/offset/topics).
/// `HERDR_STATE_DIR` overrides it; default is the launch CWD (historic
/// behavior). Launchd and manual runs MUST use the same one — split
/// directories mean replayed prompts and orphaned intents.
pub fn state_dir() -> PathBuf {
    crate::config::env_or_file("HERDR_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn focus_file() -> PathBuf {
    state_dir().join("focus.state")
}

pub(crate) fn offset_file() -> PathBuf {
    state_dir().join("offset.state")
}
