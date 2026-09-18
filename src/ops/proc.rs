//! Supervised-process primitives for `dev` (replaces dev.sh's pgrep/
//! lsof/ps/tail plumbing): guard-port probe, bot discovery, validated
//! kill, log append, status snapshot, cargo passthrough. Display goes
//! through [`super::mask`] at the print boundary.
use crate::types::{Res, SINGLE_INSTANCE_PORT};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Guard port: `.env`/env wins, else the compiled default (matches the
/// daemon bind in `main`). Callers run after `load_env_file`.
pub fn guard_port() -> u16 {
    std::env::var("HERDR_TG_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(SINGLE_INSTANCE_PORT)
}

/// Herdr socket for the status row (same default as the daemon).
pub fn herdr_socket() -> String {
    let raw = std::env::var("HERDR_SOCKET").unwrap_or_default();
    if raw.trim().is_empty() {
        let home = std::env::var("HOME").unwrap_or_default();
        return format!("{home}/.config/herdr/herdr.sock");
    }
    raw
}

/// Port busy probe: bind it ourselves — exact, no lsof. TOCTOU
/// (probe→spawn race) is accepted; the daemon's own guard bind refuses
/// second owners fail-closed.
pub fn port_busy(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_err()
}

/// PIDs that look like bot binaries (display only, never control
/// ground truth — macOS has no /proc; one read-only pgrep). Excludes
/// our own process (its argv matches the pattern while running `dev`).
pub(crate) fn bot_pids() -> Vec<u32> {
    let me = std::process::id();
    let out = std::process::Command::new("pgrep")
        .args(["-f", "target/(debug|release)/herdr-telegram"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .filter(|p| *p != me)
        .collect()
}

/// PID holding the guard port (one read-only lsof; display + validated
/// kill only).
pub(crate) fn port_pid(port: u16) -> Option<u32> {
    let out = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Full command line for kill validation (fail-closed: ambiguous PID is
/// never signalled).
fn pid_cmd(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

/// Kill validation: argv-exact daemon match (fail-closed). The daemon
/// child runs bare (`<exe>` with no subcommand), so a bare `ctl`, `dev`
/// or `ops` token in argv disqualifies — token-exact, never substring
/// (a checkout under `…/ops/…` must not wedge cleanup forever, and a
/// `ctl trigger` client in flight must never catch a TERM).
fn looks_like_bot(cmd: &str) -> bool {
    let mut toks = cmd.split_whitespace();
    let argv0 = toks.next().unwrap_or("");
    let base = argv0.rsplit('/').next().unwrap_or(argv0);
    base == "herdr-telegram" && toks.next().is_none()
}

/// Signal-safe liveness probe (kill -0, no shell).
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Stop one PID: TERM, wait ≤5s, then KILL. Refuses unless the command
/// line still matches a bot binary (stale-PID reuse kills nobody).
pub async fn stop_pid(pid: u32) -> bool {
    let Some(cmd) = pid_cmd(pid) else {
        return false;
    };
    if !looks_like_bot(&cmd) {
        return false;
    }
    let _ = tokio::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()
        .await;
    for _ in 0..5 {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if !pid_alive(pid) {
        return true;
    }
    // Re-validate before escalating: the PID may have been recycled
    // during the TERM wait — KILL must never land blind.
    if !pid_cmd(pid).is_some_and(|c| looks_like_bot(&c)) {
        return false;
    }
    let _ = tokio::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .output()
        .await;
    !pid_alive(pid)
}

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
    Ok(std::fs::OpenOptions::new().create(true).append(true).open(log)?)
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

/// Stop every bot-like PID plus the port holder. Explicit user intent
/// only (`cleanup`/`stop`) — never automatic, never prod-sweeping.
pub async fn stop_all(port: u16) -> Vec<u32> {
    let mut targets = bot_pids();
    if let Some(p) = port_pid(port)
        && !targets.contains(&p)
    {
        targets.push(p);
    }
    let mut stopped = Vec::new();
    for pid in targets {
        if pid == std::process::id() {
            continue;
        }
        if stop_pid(pid).await {
            stopped.push(pid);
        }
    }
    stopped
}

pub(crate) fn shellexpand_socket() -> String {
    let raw = herdr_socket();
    if let Some(home) = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        && let Some(rest) = raw.strip_prefix("~/")
    {
        return format!("{home}/{rest}");
    }
    raw
}

/// Last `n` lines of bot.log (read fully; logs stay small).
pub fn tail_log(n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("bot.log") else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(n);
    lines[skip..].iter().map(|l| l.to_string()).collect()
}

/// Run `cargo <args>`, returning masked tail lines + success.
pub async fn cargo(args: &[&str], home: &str) -> (Vec<String>, bool) {
    let out = tokio::process::Command::new("cargo")
        .args(args)
        .output()
        .await;
    let Ok(out) = out else {
        return (vec!["cargo not found".to_string()], false);
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
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_bot_exact() {
        // Bare daemons match; every subcommand form refuses.
        assert!(looks_like_bot("/Users/x/t/target/release/herdr-telegram"));
        assert!(looks_like_bot("./target/debug/herdr-telegram"));
        assert!(!looks_like_bot("/Users/x/t/target/debug/herdr-telegram ctl trigger w1:p1 blocked"));
        assert!(!looks_like_bot("/Users/x/t/target/debug/herdr-telegram dev"));
        assert!(!looks_like_bot("/Users/x/t/target/debug/herdr-telegram dev status"));
        assert!(!looks_like_bot("/Users/x/t/target/debug/herdr-telegram ops cleanup"));
        // Checkout path containing `ops` must not wedge cleanup.
        assert!(looks_like_bot("/Users/x/ops/herdr-telegram/target/release/herdr-telegram"));
        // Unrelated processes never match.
        assert!(!looks_like_bot("pgrep -f target/release/herdr-telegram"));
        assert!(!looks_like_bot("/usr/bin/some-daemon"));
        assert!(!looks_like_bot(""));
    }
}
