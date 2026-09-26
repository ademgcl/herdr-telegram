//! Private file writes + unique tmp siblings for tmp+rename state
//! files. Split from `types` (300-line file limit); re-exported there
//! so existing `crate::types::{write_private, chmod_private,
//! unique_tmp}` call sites keep working unchanged.

/// Private file write (0600 on unix): state files carry chat IDs and
/// prompt excerpts — dev.sh's `umask 077` covered them; this is its
/// replacement now that ops lives in Rust. Tmp+rename callers pass the
/// tmp path (rename preserves the mode).
pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        // Create-only mode is not enough: a pre-existing 0644 tmp
        // (crash leftover) would keep its mode through rename.
        let mut perm = f.metadata()?.permissions();
        perm.set_mode(0o600);
        f.set_permissions(perm)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Best-effort 0600 on an existing file (unix): `fs::copy` backups
/// inherit the umask (≈0644) while the live files are 0600 — a corrupt
/// backup carrying chat IDs + prompt excerpts must not stay readable.
pub(crate) fn chmod_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perm = meta.permissions();
            perm.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Unique tmp sibling for tmp+rename writers (single source).
/// Concurrent saves (two panes settling at once) sharing one `<path>.tmp`
/// truncate + interleave + rename garbage — the rename then atomically
/// installs the tear. Per-call names (pid + counter) keep renames atomic
/// without a cross-task mutex. Stale tmps from a crash are inert
/// (never read).
pub(crate) fn unique_tmp(path: &std::path::Path) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}-{n}", std::process::id()));
    std::path::PathBuf::from(tmp)
}

/// Corrupt-backup filename verdict (pure, tested): `true` when `name`
/// is a `.corrupt-*.bak` sibling of `base` (same `<base>.corrupt-`
/// prefix, `.bak` suffix). Single source for the prune below so the
/// match can never drift between writers.
pub(crate) fn is_corrupt_backup(base: &str, name: &str) -> bool {
    let prefix = format!("{base}.corrupt-");
    name.starts_with(&prefix) && name.ends_with(".bak") && name.len() > prefix.len() + 4
}

/// Cap `.corrupt-*.bak` siblings (single source for jobs/topics
/// writers): every corrupt load copies one backup with no expiry
/// otherwise — keep the newest `keep`, delete the rest. Best-effort.
pub(crate) fn prune_corrupt_backups(path: &std::path::Path, keep: usize) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(base) = path.file_name().and_then(|b| b.to_str()) else {
        return;
    };
    let Ok(dir) = std::fs::read_dir(parent) else {
        return;
    };
    let mut hits: Vec<std::path::PathBuf> = Vec::new();
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if is_corrupt_backup(base, &name) {
            hits.push(e.path());
        }
    }
    if hits.len() <= keep {
        return;
    }
    // Sort by mtime (newest last), tie-break path: lexicographic order
    // on `corrupt-<secs>[-<pid>].bak` names keeps an OLD high-epoch
    // name forever when pids/time disagree — the prune must keep the
    // chronologically newest backups regardless of name shape.
    let mut hits: Vec<(std::time::SystemTime, std::path::PathBuf)> = hits
        .into_iter()
        .map(|p| {
            let m = std::fs::metadata(&p)
                .and_then(|md| md.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (m, p)
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, old) in hits.iter().take(hits.len() - keep) {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_corrupt_backup_match() {
        assert!(is_corrupt_backup(
            "jobs.state",
            "jobs.state.corrupt-1-2.bak"
        ));
        assert!(is_corrupt_backup("jobs.state", "jobs.state.corrupt-1.bak"));
        assert!(!is_corrupt_backup("jobs.state", "jobs.state.prev"));
        assert!(!is_corrupt_backup("jobs.state", "jobs.state.corrupt-1.tmp"));
        assert!(!is_corrupt_backup("jobs.state", "other.corrupt-1.bak"));
    }

    #[test]
    fn test_prune_corrupt_backups_keeps_newest() {
        let dir = std::env::temp_dir().join(format!(
            "ht-prune-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("jobs.state");
        std::fs::write(&base, "{}").unwrap();
        for i in 0..7 {
            let p = dir.join(format!("jobs.state.corrupt-{i}.bak"));
            std::fs::write(&p, "x").unwrap();
        }
        prune_corrupt_backups(&base, 5);
        let left: usize = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| is_corrupt_backup("jobs.state", &e.file_name().to_string_lossy()))
            .count();
        assert_eq!(left, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prune_corrupt_backups_mtime_not_lexicographic() {
        // Lexicographic order on the name would keep the HIGH epoch
        // forever: `corrupt-9999999999` (created first) sorts after
        // `corrupt-10000000000` (created second). mtime order keeps the
        // chronologically newer backup regardless of name shape.
        let dir = std::env::temp_dir().join(format!(
            "ht-prune-mtime-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("jobs.state");
        std::fs::write(&base, "{}").unwrap();
        let old_name = dir.join("jobs.state.corrupt-9999999999-9.bak");
        std::fs::write(&old_name, "old").unwrap();
        // Ensure a strictly newer mtime (some filesystems have 1s
        // granularity).
        std::thread::sleep(std::time::Duration::from_millis(20));
        let new_name = dir.join("jobs.state.corrupt-10000000000-1.bak");
        std::fs::write(&new_name, "new").unwrap();
        prune_corrupt_backups(&base, 1);
        assert!(
            !old_name.exists(),
            "lexicographically-newer but chronologically-older backup must go"
        );
        assert!(
            new_name.exists(),
            "chronologically newest must survive keep=1"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
