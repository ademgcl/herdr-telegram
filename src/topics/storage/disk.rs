//! Atomic disk layer for topic mappings: tmp+rename main, tmp+rename
//! `.prev` last-good, corrupt→`.bak` + prev fallback. Split from `mod`
//! (300-line file limit).
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct Store {
    #[serde(default)]
    pub topics: HashMap<String, i64>,
    #[serde(default)]
    pub unread: HashSet<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
    #[serde(default)]
    pub titles: HashMap<String, String>,
    #[serde(default)]
    pub pins: HashMap<String, i64>,
    #[serde(default)]
    pub icons: HashMap<String, String>,
    #[serde(default)]
    pub last_msgs: HashMap<String, Vec<i64>>,
}

pub(crate) fn prev_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.prev", path.display()))
}

fn load_prev(path: &Path) -> Option<Store> {
    let prev = prev_path(path);
    if prev == *path {
        return None;
    }
    let ptxt = fs::read_to_string(&prev).ok()?;
    if let Ok(s) = serde_json::from_str::<Store>(&ptxt) {
        return Some(s);
    }
    serde_json::from_str::<HashMap<String, i64>>(&ptxt)
        .ok()
        .map(|topics| Store {
            topics,
            ..Default::default()
        })
}

pub(crate) fn read_store(path: &Path) -> Store {
    let txt = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(s) = load_prev(path) {
                eprintln!("[topics] main missing, restored previous-good backup");
                return s;
            }
            return Store::default();
        }
    };
    // Legacy flat map first: else it parses as empty Store and wipes all.
    if let Ok(topics) = serde_json::from_str::<HashMap<String, i64>>(&txt) {
        return Store {
            topics,
            ..Default::default()
        };
    }
    match serde_json::from_str::<Store>(&txt) {
        Ok(s) => s,
        Err(_) => {
            if let Some(s) = load_prev(path) {
                eprintln!("[topics] state corrupt, restored previous-good backup");
                return s;
            }
            if !txt.trim().is_empty() {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let bak = PathBuf::from(format!("{}.corrupt-{}.bak", path.display(), secs));
                let _ = fs::copy(path, &bak);
            }
            Store::default()
        }
    }
}

pub(crate) fn write_store(path: &Path, s: &Store, backup: bool) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(s) else {
        return;
    };
    // Durable tmp+rename: fsync file before rename + dir after, so a
    // power loss can't leave a torn main (best effort, never panics).
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if fs::write(&tmp, json).is_ok() {
        if let Ok(f) = fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        match fs::rename(&tmp, path) {
            Ok(()) => {
                if let Some(parent) = path.parent()
                    && let Ok(d) = fs::File::open(parent)
                {
                    let _ = d.sync_all();
                }
                if !backup {
                    return;
                }
                // Atomic backup, never a torn copy.
                let prev = prev_path(path);
                let mut ptmp = prev.as_os_str().to_owned();
                ptmp.push(".tmp");
                let ptmp = PathBuf::from(ptmp);
                if fs::write(&ptmp, serde_json::to_string_pretty(s).unwrap_or_default()).is_ok() {
                    if let Ok(f) = fs::File::open(&ptmp) {
                        let _ = f.sync_all();
                    }
                    let _ = fs::rename(&ptmp, &prev);
                    if let Some(parent) = prev.parent()
                        && let Ok(d) = fs::File::open(parent)
                    {
                        let _ = d.sync_all();
                    }
                }
            }
            Err(e) => eprintln!("[topics] rename {} failed: {e}", path.display()),
        }
    }
}
