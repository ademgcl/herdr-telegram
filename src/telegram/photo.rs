//! Incoming photo fetch: `getFile` → download → private file under
//! `<state_dir>/img/`, newest 20 kept. Split by layer: raw download in
//! `client`, routing/prompt composition in `handlers::photo`.
use crate::state::AppState;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

/// Photos kept on disk (prune bound — screenshots accumulate fast and
/// state must never grow unbounded).
pub(crate) const PHOTO_KEEP: usize = 20;

/// Largest size's `file_id` (pure, tested): Telegram orders `photo`
/// ascending, so the last element is the best quality.
pub(crate) fn largest_file_id(photo: &serde_json::Value) -> Option<String> {
    photo
        .as_array()
        .and_then(|a| a.last())
        .and_then(|s| s["file_id"].as_str())
        .map(str::to_string)
}

/// Fetch one photo to disk. `Some(path)` absolute on success (agents
/// run in their own cwd — a relative path would resolve nowhere);
/// `None` on any failure (the caller refuses visibly, never prompts
/// caption-only and silently drops the image).
pub async fn fetch_photo(s: &AppState, file_id: &str) -> Option<PathBuf> {
    let meta =
        s.tg.call(
            "getFile",
            json!({"file_id": file_id}),
            Duration::from_secs(15),
        )
        .await
        .ok()?;
    let path = meta["file_path"].as_str()?;
    let bytes = s.tg.download_file(path).await.ok()?;
    let dest = img_path()?;
    if crate::types::write_private(&dest, &bytes).is_err() {
        return None;
    }
    prune_photos();
    // Absolute for the agent: it resolves relative to its own cwd.
    std::fs::canonicalize(&dest).ok().or(Some(dest))
}

/// Photo dir under the state root (tracked-file hygiene: images land
/// beside state, never in the repo — `.gitignore` covers `img/` like
/// every `*.state` file; fail-closed `None` on any error).
fn img_dir() -> Option<PathBuf> {
    let dir = crate::state::state_dir().join("img");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Unique name, newest-sortable (`nanos` first — prune keeps the
/// largest 20 without touching mtimes). `None` when the clock is
/// unreadable (never mint colliding names).
fn img_path() -> Option<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(img_dir()?.join(format!("photo-{nanos:020}-{}.jpg", std::process::id())))
}

/// Prune survivors (pure, tested): keep the newest `PHOTO_KEEP` names.
pub(crate) fn prune_names(names: &mut Vec<String>) -> Vec<String> {
    names.sort();
    if names.len() > PHOTO_KEEP {
        let drop = names.len() - PHOTO_KEEP;
        names.drain(..drop).collect()
    } else {
        Vec::new()
    }
}

/// Drop pruned files best-effort (a leftover ages out on the next
/// photo — never fail the current fetch over cleanup).
fn prune_photos() {
    let Some(dir) = img_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for dead in prune_names(&mut names) {
        let _ = std::fs::remove_file(dir.join(dead));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_largest_file_id_takes_last() {
        let photo = json!([
            {"file_id": "small", "width": 90},
            {"file_id": "mid", "width": 320},
            {"file_id": "big", "width": 1280},
        ]);
        assert_eq!(largest_file_id(&photo).as_deref(), Some("big"));
        assert_eq!(largest_file_id(&json!([])), None);
        assert_eq!(largest_file_id(&json!(null)), None);
        assert_eq!(largest_file_id(&json!([{"width": 1}])), None);
    }

    #[test]
    fn test_prune_names_keeps_newest_twenty() {
        let mut names: Vec<String> = (0..25).map(|i| format!("photo-{i:020}-1.jpg")).collect();
        let dead = prune_names(&mut names);
        assert_eq!(dead.len(), 5);
        assert_eq!(names.len(), PHOTO_KEEP);
        assert!(names.iter().all(|n| !dead.contains(n)));
        assert!(names.contains(&format!("photo-{:020}-1.jpg", 24)));
        // Under the cap: nothing pruned.
        let mut few = vec!["a".to_string()];
        assert!(prune_names(&mut few).is_empty());
    }
}
