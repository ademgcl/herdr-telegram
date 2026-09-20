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
