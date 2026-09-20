//! Validated process discovery + kill for `dev` (split from `proc`:
//! 300-line file limit). `proc` re-exports the names it owned, so
//!! `proc::bot_pids()` call sites stay untouched.
#![deny(missing_docs)]

/// PIDs that look like bot binaries (display only, never control
/// ground truth — macOS has no /proc; one read-only pgrep). Excludes
/// our own process (its argv matches the pattern while running `dev`).
/// pgrep -f over-matches one-shot CLI invocations (`herdr-telegram ctl
/// …`, `dev …`), so every candidate is re-validated through
/// `looks_like_bot` (bare daemon argv only) — discovery and kill
/// validation never disagree, or `dev status` reports a transient CLI
/// pid as a running bot.
pub(crate) fn bot_pids() -> Vec<u32> {
    let me = std::process::id();
    // Broad pre-filter: installed/launchd prod binaries never live under
    // `target/` — `looks_like_bot` below re-validates the bare daemon
    // argv, so one-shot `ctl`/`dev` invocations never count.
    let out = std::process::Command::new("pgrep")
        .args(["-f", "herdr-telegram"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .filter(|p| *p != me)
        .filter(|pid| pid_cmd(*pid).is_some_and(|c| looks_like_bot(&c)))
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
    if cmd.is_empty() { None } else { Some(cmd) }
}

/// Kill validation: argv-exact daemon match (fail-closed). The daemon
/// child runs bare (`<exe>` with no subcommand), so a bare `ctl`, `dev`
/// or `ops` token in argv disqualifies — token-exact, never substring
/// (a checkout under `…/ops/…` must not wedge cleanup forever, and a
/// `ctl trigger` client in flight must never catch a TERM).
pub(crate) fn looks_like_bot(cmd: &str) -> bool {
    // Suffix match, never argv0-token split: an install path with spaces
    // (`/Users/a b/…/herdr-telegram`) splits argv0 wrong and the daemon
    // is never matched (status says Stopped, cleanup misses it). The
    // daemon runs bare (no subcommand), so exactly-exe is the whole
    // line — any trailing `ctl`/`dev`/`ops` token disqualifies (the
    // suffix stops matching). A `pgrep -f …herdr-telegram` line also
    // ends in the exe name but starts with a bare argv word, never a
    // path — the prefix must look path-like when it holds spaces.
    if cmd == "herdr-telegram" {
        return true;
    }
    let Some(prefix) = cmd.strip_suffix("/herdr-telegram") else {
        return false;
    };
    if !prefix.chars().any(char::is_whitespace) {
        return true;
    }
    (prefix.starts_with('/') || prefix.starts_with('.') || prefix.starts_with('~'))
        && !prefix.contains(" -")
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
