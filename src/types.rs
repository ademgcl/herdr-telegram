pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TG_POLL_SECS: i64 = 25;
pub const STALE_SECS: u64 = 600;
/// Setup-note window: an unconfigured group reminds once a day —
/// time-bounded like stale notices, never forever-mute, never spam.
pub const NAGGED_SECS: u64 = 86400;
pub const LIVE_EDIT_COOLDOWN_SECS: u64 = 4;
/// Bound for silent live RPCs (stream edits/sends, handoff retire):
/// flood-wait retries must not stall settle past the tick — a miss
/// retries next tick. Single source for jobs + telegram (distinct from
/// the edit cooldown above, same seconds by design).
pub const LIVE_RPC_TIMEOUT_SECS: u64 = 4;
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

/// Mask $HOME anywhere in a display string (chat/log hygiene: herdr
/// errors carry socket/cwd paths with the username). Boundary-aware:
/// `/Users/x2` never matches `/Users/x`; only a trailing `/` or
/// end-of-string counts. Pure so it is unit-tested.
///
/// Scope note: HOME-only by design (error strings keep their reasons).
/// Full display masking (tokens, chat IDs) lives in
/// `ops::mask::mask_line` for console output — don't merge the two.
pub fn mask_home_with(s: &str, home: &str) -> String {
    let home = home
        .strip_suffix('/')
        .filter(|h| !h.is_empty())
        .unwrap_or(home);
    if home.is_empty() || home == "/" {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(home) {
        let after = &rest[i + home.len()..];
        if after.starts_with('/') || after.is_empty() {
            out.push_str(&rest[..i]);
            out.push('~');
            rest = after;
        } else {
            out.push_str(&rest[..i + home.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Process $HOME (empty when unset). Single source for every
/// `env::var("HOME")` read — dup'd literals re-drift (one site once
/// read `HOM` and every path silently fell back to CWD-relative).
pub fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// Single source for the unset-HOME refusal (launchd install paths).
pub const HOME_NOT_SET: &str = "HOME not set";

/// [`mask_home_with`] with the process HOME (chat/log call sites).
pub fn mask_home(s: &str) -> String {
    mask_home_with(s, &home_dir())
}

/// Render a path for logs with $HOME collapsed to `~` (no username
/// leak). Single source for main's boot log line.
pub fn home_masked(p: &std::path::Path) -> String {
    collapse_home(&p.display().to_string(), &home_dir())
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

/// Pane-id shape for `focus.state` (fail-closed load: garbage, torn
/// writes, and hand-edits must never become the DM routing focus).
/// Herdr pane ids are `ws:pane` (`w8:p1`) — the colon is the shape.
/// Pure so it is unit-tested.
pub fn valid_focus(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.contains(':')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_home() {
        assert_eq!(
            collapse_home("/Users/x/.config/a", "/Users/x"),
            "~/.config/a"
        );
        assert_eq!(collapse_home("/other/path", "/Users/x"), "/other/path");
        assert_eq!(collapse_home("/Users/x/a", ""), "/Users/x/a");
        // Anchored: mid-path occurrences stay untouched (HOME=/ still
        // collapses sanely instead of corrupting the leading slash).
        assert_eq!(
            collapse_home("/tmp/Users/x/a", "/Users/x"),
            "/tmp/Users/x/a"
        );
        assert_eq!(collapse_home("/Users/x2/a", "/Users/x"), "/Users/x2/a");
        assert_eq!(collapse_home("/etc/hosts", "/"), "~/etc/hosts");
        assert_eq!(collapse_home("/Users/x", "/Users/x/"), "~");
        assert_eq!(collapse_home("/Users/x/a", "/Users/x/"), "~/a");
    }

    #[test]
    fn test_mask_home_with_mid_string_and_boundary() {
        // Mid-string socket/cwd paths mask (chat herdr errors); the
        // reason survives, the username doesn't.
        assert_eq!(
            mask_home_with("dial /Users/x/.config/herdr.sock: refused", "/Users/x"),
            "dial ~/.config/herdr.sock: refused"
        );
        assert_eq!(
            mask_home_with("a /Users/x/b c /Users/x/d", "/Users/x"),
            "a ~/b c ~/d"
        );
        // Mid-name prefix never masks; empty/root home leaves alone.
        assert_eq!(mask_home_with("/Users/x2/a", "/Users/x"), "/Users/x2/a");
        assert_eq!(mask_home_with("/Users/x/a", ""), "/Users/x/a");
        assert_eq!(mask_home_with("/etc/hosts", "/"), "/etc/hosts");
        assert_eq!(mask_home_with("no paths here", "/Users/x"), "no paths here");
    }

    #[test]
    fn test_valid_focus_shape() {
        assert!(valid_focus("w8:p1"));
        assert!(valid_focus("t:p1"));
        // Garbage / torn writes / hand-edits never become DM focus.
        assert!(!valid_focus(""));
        assert!(!valid_focus("???"));
        assert!(!valid_focus("bogus"));
        assert!(!valid_focus("w8:p1\nw8:p2"));
        assert!(!valid_focus("w8 p1:x"));
        assert!(!valid_focus(&"w".repeat(300)));
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

    #[test]
    fn test_home_masked_collapses_home_prefix() {
        // Single source for main's boot log line: $HOME collapses to `~`.
        assert_eq!(
            collapse_home("/Users/x/.config/ht", "/Users/x"),
            "~/.config/ht"
        );
        assert_eq!(
            home_masked(std::path::Path::new("/other/path")),
            collapse_home("/other/path", &home_dir())
        );
    }
}
