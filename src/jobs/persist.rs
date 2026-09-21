//! Durable prompt intent: which pane owes a reply where. Recorded on
//! every submit, cleared on settle/cancel — boot re-arms watchers from
//! it so restarts stop eating replies.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPrompt {
    pub chat: i64,
    pub thread: Option<i64>,
    pub prompt: String,
    pub started_unix: u64,
}

pub fn store_path() -> PathBuf {
    crate::state::state_dir().join("jobs.state")
}

fn prev_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.prev", path.display()))
}

fn load_prev(path: &Path) -> Option<HashMap<String, PendingPrompt>> {
    let prev = prev_path(path);
    if prev == *path {
        return None;
    }
    let txt = std::fs::read_to_string(&prev).ok()?;
    serde_json::from_str(&txt).ok()
}

pub fn load_file(path: &Path) -> HashMap<String, PendingPrompt> {
    let Ok(txt) = std::fs::read_to_string(path) else {
        // Missing main (crash, disk-full rename): last-good backup
        // restores owed replies instead of wiping them (topics parity).
        if let Some(m) = load_prev(path) {
            eprintln!("[jobs] intent main missing, restored previous-good backup");
            return m;
        }
        return HashMap::new();
    };
    if txt.trim().is_empty() {
        // Empty main is corruption, not a wipe (topics parity).
        if let Some(m) = load_prev(path) {
            eprintln!("[jobs] intent main empty, restored previous-good backup");
            return m;
        }
        return HashMap::new();
    }
    serde_json::from_str(&txt).unwrap_or_else(|_| {
        // Corrupt main: last-good backup first, `.corrupt-*.bak` only
        // when no backup exists — a truncated write must never silently
        // eat owed replies.
        if let Some(m) = load_prev(path) {
            eprintln!("[jobs] intent corrupt, restored previous-good backup");
            return m;
        }
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
        HashMap::new()
    })
}

pub fn save_file(path: &Path, map: &HashMap<String, PendingPrompt>) {
    if let Ok(json) = serde_json::to_string_pretty(map) {
        // Unique tmp (never shared `<path>.tmp`): concurrent saves from
        // two settling panes must not interleave into one torn file.
        let tmp = crate::types::unique_tmp(path);
        if crate::types::write_private(&tmp, json.as_bytes()).is_ok() {
            if let Ok(f) = std::fs::File::open(&tmp) {
                let _ = f.sync_all();
            }
            if std::fs::rename(&tmp, path).is_err() {
                eprintln!("[jobs] intent rename failed (disk full?)");
                return;
            }
            if let Some(parent) = path.parent()
                && let Ok(d) = std::fs::File::open(parent)
            {
                let _ = d.sync_all();
            }
            // Atomic last-good backup, never a torn copy (topics parity).
            let prev = prev_path(path);
            let ptmp = crate::types::unique_tmp(&prev);
            if crate::types::write_private(&ptmp, json.as_bytes()).is_ok() {
                if let Ok(f) = std::fs::File::open(&ptmp) {
                    let _ = f.sync_all();
                }
                let _ = std::fs::rename(&ptmp, &prev);
            }
        } else {
            eprintln!("[jobs] intent write failed (disk full?)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unique per call (pid + counter, test_tag parity): pid alone
    // collides across runs on pid recycle — a leftover file breaks the
    // `load_file(&p).is_empty()` openers below.
    fn tmp_path(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PathBuf::from(format!(
            "/tmp/herdr-tg-test-jobs-{tag}-{}-{n}.json",
            std::process::id()
        ))
    }

    #[test]
    fn test_roundtrip_and_corrupt() {
        let p = tmp_path("roundtrip");
        assert!(load_file(&p).is_empty());
        let mut m = HashMap::new();
        m.insert(
            "w8:p1".to_string(),
            PendingPrompt {
                chat: 1,
                thread: Some(6),
                prompt: "hi".into(),
                started_unix: 42,
            },
        );
        save_file(&p, &m);
        let back = load_file(&p);
        assert_eq!(back["w8:p1"].prompt, "hi");
        assert_eq!(back["w8:p1"].thread, Some(6));
        std::fs::write(&p, "not json{{").unwrap();
        // Last-good backup restores the owed reply (topics parity).
        let back = load_file(&p);
        assert_eq!(back["w8:p1"].prompt, "hi");
        // With no backup either, corrupt still drops to empty + .bak.
        let _ = std::fs::remove_file(prev_path(&p));
        assert!(load_file(&p).is_empty());
        // Corrupt loads back the file up — clean up strays, not just `p`.
        let prefix = format!("{}.corrupt-", p.display());
        if let Ok(dir) = std::fs::read_dir("/tmp") {
            for e in dir.flatten() {
                let name = e.path().display().to_string();
                if name.starts_with(&prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(prev_path(&p));
    }

    #[test]
    fn test_empty_main_restores_prev() {
        let p = tmp_path("prev");
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(prev_path(&p));
        let mut m = HashMap::new();
        m.insert(
            "w8:p2".to_string(),
            PendingPrompt {
                chat: 2,
                thread: None,
                prompt: "yo".into(),
                started_unix: 7,
            },
        );
        save_file(&p, &m);
        std::fs::write(&p, "").unwrap();
        assert_eq!(load_file(&p)["w8:p2"].prompt, "yo");
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(prev_path(&p));
    }
}
