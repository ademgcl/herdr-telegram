pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TG_POLL_SECS: i64 = 25;
pub const STALE_SECS: u64 = 600;
pub const LIVE_EDIT_COOLDOWN_SECS: u64 = 4;
pub const MAX_MSG_UNITS: usize = 3900;
pub const SINGLE_INSTANCE_PORT: u16 = 47319;
/// Herdr socket protocol this bot is built against (see `herdr api schema`).
pub const HERDR_PROTOCOL: u64 = 22;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRow {
    pub kind: String,
    pub pane: String,
    pub title: String,
    pub status: String,
    pub ws: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDetail {
    pub kind: String,
    pub pane: String,
    pub title: String,
    pub status: String,
    pub ws: String,
    pub cwd: String,
    pub branch: Option<String>,
}

impl From<AgentDetail> for AgentRow {
    fn from(d: AgentDetail) -> Self {
        Self {
            kind: d.kind,
            pane: d.pane,
            title: d.title,
            status: d.status,
            ws: d.ws,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub label: String,
    pub number: u64,
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    pub chat_id: i64,
    pub message_thread_id: Option<i64>,
    pub text: String,
}

/// Collapse $HOME to `~` in a display path (log hygiene: no username
/// leak). Pure so it is unit-tested; empty home leaves the path alone.
pub fn collapse_home(path: &str, home: &str) -> String {
    if home.is_empty() {
        path.to_string()
    } else {
        // Anchored on a separator boundary: an unanchored replace masks
        // mid-path substrings (`/Users/x2` ≠ `/Users/x`) and corrupts
        // HOME=/ (every absolute path "starts" with it). Trailing-slash
        // homes are trimmed first (root stays root).
        let home = home
            .strip_suffix('/')
            .filter(|h| !h.is_empty())
            .unwrap_or(home);
        match path.strip_prefix(home) {
            // The home dir itself.
            Some("") => "~".to_string(),
            // Normal rest keeps its leading slash ("/.config" → "~/.config").
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            // Trailing-slash home or root ("/"): the rest lacks the
            // separator, but the boundary is real — re-add it.
            Some(rest) if home.ends_with('/') => format!("~/{rest}"),
            // Mid-name prefix (`/Users/x2` vs `/Users/x`): not home.
            _ => path.to_string(),
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_home() {
        assert_eq!(collapse_home("/Users/x/.config/a", "/Users/x"), "~/.config/a");
        assert_eq!(collapse_home("/other/path", "/Users/x"), "/other/path");
        assert_eq!(collapse_home("/Users/x/a", ""), "/Users/x/a");
        // Anchored: mid-path occurrences stay untouched (HOME=/ still
        // collapses sanely instead of corrupting the leading slash).
        assert_eq!(collapse_home("/tmp/Users/x/a", "/Users/x"), "/tmp/Users/x/a");
        assert_eq!(collapse_home("/Users/x2/a", "/Users/x"), "/Users/x2/a");
        assert_eq!(collapse_home("/etc/hosts", "/"), "~/etc/hosts");
        assert_eq!(collapse_home("/Users/x", "/Users/x/"), "~");
        assert_eq!(collapse_home("/Users/x/a", "/Users/x/"), "~/a");
    }

    #[test]
    fn test_write_private_roundtrip() {
        let p = std::env::temp_dir().join(format!("ht-priv-{}.txt", std::process::id()));
        write_private(&p, b"secret").expect("write");
        assert_eq!(std::fs::read(&p).expect("read"), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).expect("meta").permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(&p);
    }
}
