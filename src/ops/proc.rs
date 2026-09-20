//! Supervised-process primitives for `dev` (replaces dev.sh's pgrep/
//! lsof/ps/tail plumbing): guard-port probe, bot discovery, validated
//! kill, log append, status snapshot, cargo passthrough. Display goes
//! through [`super::mask`] at the print boundary.
use crate::types::{Res, SINGLE_INSTANCE_PORT};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Guard port: `.env`/env wins, else the compiled default. Single source
/// with the daemon bind in `main` — an invalid `HERDR_TG_PORT` fails
/// loudly everywhere (a silent fallback would run ops against a different
/// port than the daemon: split-brain).
pub fn guard_port() -> Res<u16> {
    match crate::config::env_or_file("HERDR_TG_PORT") {
        Some(v) => {
            let port: u16 =
                v.trim()
                    .parse()
                    .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                        "HERDR_TG_PORT invalid (must be a port number)".into()
                    })?;
            // Port 0 binds an OS-assigned ephemeral port, which always
            // succeeds — the single-instance guard would never fire and
            // two daemons would run side by side. Reject fail-closed.
            if port == 0 {
                return Err("HERDR_TG_PORT invalid (must be a port number)".into());
            }
            Ok(port)
        }
        None => Ok(SINGLE_INSTANCE_PORT),
    }
}

/// Herdr socket for the status row (same default + `~/` expansion as the daemon).
/// Single source: callers use this directly (no wrapper).
pub fn herdr_socket() -> String {
    let raw = crate::config::env_or_file("HERDR_SOCKET").unwrap_or_default();
    let v = raw.trim();
    let home = crate::types::home_dir();
    // No HOME to expand against: return raw, never `/.config/…` (a root
    // path the daemon never dials — the status row would probe wrong).
    if home.is_empty() {
        return v.to_string();
    }
    if v.is_empty() {
        return format!("{home}/.config/herdr/herdr.sock");
    }
    match v.strip_prefix("~/") {
        // Bare `~/` would expand to `$HOME/` (a directory the daemon
        // rejects fail-closed): show the raw value, never a silently
        // expanded dir the status row would probe wrong.
        Some("") => v.to_string(),
        Some(rest) => format!("{home}/{rest}"),
        None => v.to_string(),
    }
}

/// Port busy probe: bind it ourselves — exact, no lsof. TOCTOU
/// (probe→spawn race) is accepted; the daemon's own guard bind refuses
/// second owners fail-closed.
pub fn port_busy(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_err()
}

// Validated discovery + kill live in `proc_kill` (300-line file limit);
// re-exported so `proc::bot_pids()` call sites stay untouched.
#[cfg(test)]
pub(crate) use super::proc_kill::looks_like_bot;
pub use super::proc_kill::stop_all;
pub(crate) use super::proc_kill::{bot_pids, port_pid};

/// Spawn the supervised bot: current exe, stdio into `bot.log`
/// (append, 0600 on create), stdin null. Caller owns the Child.
pub fn spawn_bot(log: &Path) -> Res<tokio::process::Child> {
    let exe = std::env::current_exe()?;
    let file = open_log(log)?;
    let err = open_log(log)?;
    let child = tokio::process::Command::new(exe)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err)
        .kill_on_drop(true)
        .spawn()?;
    Ok(child)
}

#[cfg(unix)]
fn open_log(log: &Path) -> Res<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)?;
    // Create-only mode is not enough: chmod pre-existing files too, or
    // a 0644 bot.log keeps leaking appended chat IDs and excerpts.
    let mut perm = f.metadata()?.permissions();
    perm.set_mode(0o600);
    f.set_permissions(perm)?;
    Ok(f)
}

#[cfg(not(unix))]
fn open_log(log: &Path) -> Res<std::fs::File> {
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?)
}

/// Graceful supervised stop: TERM first (the daemon flushes its poll
/// offset on TERM — a KILL mid-`handle_update` replays the update as a
/// duplicate agent submit), 5s wait, then KILL. Dev.sh TERM→wait→KILL
/// parity for restarts and signal shutdowns.
pub async fn stop_graceful(child: &mut tokio::process::Child) {
    let Some(pid) = child.id().map(|p| p.to_string()) else {
        let _ = child.wait().await;
        return;
    };
    let _ = tokio::process::Command::new("kill")
        .args(["-TERM", &pid])
        .output()
        .await;
    for _ in 0..5 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            _ => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Run `cargo <args>`, returning masked tail lines + success.
pub async fn cargo(args: &[&str], home: &str) -> (Vec<String>, bool) {
    // Spawn (never `timeout(output())`): dropping an `output()` future
    // leaves the child running detached — a wedged cargo would leak.
    let Ok(child) = tokio::process::Command::new("cargo")
        .args(args)
        .kill_on_drop(true)
        .spawn()
    else {
        return (vec!["cargo not found".to_string()], false);
    };
    // Bounded: a hung build fails visibly, never wedges `dev` forever.
    let out = match tokio::time::timeout(
        std::time::Duration::from_secs(300),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        _ => return (vec!["cargo timed out".to_string()], false),
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(15);
    let masked = lines[skip..]
        .iter()
        .map(|l| super::mask::mask_line(l, home))
        .collect();
    (masked, out.status.success())
}

/// Bot log path (repo cwd, like the daemon's-relative state files).
pub fn log_path() -> PathBuf {
    PathBuf::from("bot.log")
}

#[cfg(test)]
#[path = "proc_tests.rs"]
mod tests;
