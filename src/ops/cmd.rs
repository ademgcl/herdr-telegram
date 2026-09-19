//! One-shot ops commands (status, log follow, foreground runs). Display
//! is masked at print time via [`super::say`]; files untouched.
use super::{mask, proc, say};
use crate::types::Res;
use std::os::unix::fs::FileTypeExt;

/// Status snapshot (raw; masked at print).
pub struct Status {
    pub bot_pids: Vec<u32>,
    pub port_pid: Option<u32>,
    pub socket_ok: bool,
    pub offset: Option<String>,
    pub topic_titles: usize,
    pub log_bytes: u64,
}

fn snapshot(port: u16) -> Status {
    let socket = proc::shellexpand_socket();
    // State files live in HERDR_STATE_DIR (or repo cwd), like the daemon
    // sees them — never bare `./` when redirected.
    let dir = crate::state::state_dir();
    Status {
        bot_pids: proc::bot_pids(),
        port_pid: proc::port_pid(port),
        socket_ok: std::fs::metadata(&socket)
            .map(|m| m.file_type().is_socket())
            .unwrap_or(false),
        offset: std::fs::read_to_string(dir.join("offset.state"))
            .ok()
            .map(|s| s.trim().to_string()),
        topic_titles: std::fs::read_to_string(dir.join("topics.state"))
            .map(|t| t.lines().filter(|l| l.contains("\":")).count())
            .unwrap_or(0),
        log_bytes: std::fs::metadata("bot.log").map(|m| m.len()).unwrap_or(0),
    }
}

pub(crate) fn print_status(port: u16) {
    let home = crate::types::home_dir();
    let st = snapshot(port);
    say(&home, "=== HERDR TELEGRAM STATUS ===");
    if st.bot_pids.is_empty() {
        say(&home, "  Bot Status:   Stopped");
    } else {
        say(
            &home,
            &format!(
                "  Bot Status:   Running (PID: {})",
                st.bot_pids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
    }
    match st.port_pid {
        Some(p) => say(&home, &format!("  Guard Port:   In use :{port} (PID: {p})")),
        None => say(&home, &format!("  Guard Port:   Free :{port}")),
    }
    say(
        &home,
        &format!(
            "  Herdr Socket: {} ({})",
            if st.socket_ok {
                "Available"
            } else {
                "Not found"
            },
            proc::herdr_socket()
        ),
    );
    if let Some(off) = st.offset {
        say(&home, &format!("  Offset State: {off}"));
    }
    say(
        &home,
        &format!("  Topics State: ~{} titles", st.topic_titles),
    );
    say(
        &home,
        &format!("  Log File:     bot.log ({} bytes)", st.log_bytes),
    );
    say(&home, "=============================");
}

/// Follow bot.log from its current end (callers print history first).
/// Shares the caller's stdin channel — a second reader would race it
/// for `q` lines. Partial trailing lines held for the next poll.
/// Seek-based: the file grows unbounded over long prod runs (rotation
/// happens only at boot), so re-reading it whole every 500ms would
/// burn RAM/CPU — read only the appended window, capped per poll.
pub(crate) async fn follow_log(home: &str, rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) {
    println!("following bot.log — `q` + Enter exits (masking on)");
    // Cap per-poll appends: a burst bigger than this jumps the cursor
    // (the tail command covers history; follow is for live lines).
    const POLL_CAP: u64 = 256 * 1024;
    let mut pos = std::fs::metadata(proc::log_path())
        .map(|m| m.len())
        .unwrap_or(0);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                let len = std::fs::metadata(proc::log_path()).map(|m| m.len()).unwrap_or(pos);
                if len <= pos {
                    pos = len;
                } else if let Ok((bytes, at)) = read_tail_from(&proc::log_path(), pos, len, POLL_CAP) {
                    pos = at;
                    // Hold back a partial trailing line for next poll.
                    let end = bytes.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
                    if end > 0 {
                        print!("{}", mask::mask_line(&String::from_utf8_lossy(&bytes[..end]), home));
                        pos += end as u64;
                    }
                }
            }
            line = rx.recv() => {
                if line.as_deref().map(str::trim) != Some("q") && line.is_some() {
                    continue;
                }
                break;
            }
        }
    }
}

/// Read at most `cap` bytes of `path` ending at `len`, starting from
/// `pos` (seek — never a full-file read). Returns the bytes plus the
/// cursor they start at: when the gap exceeds the cap the cursor jumps
/// (a partial first line may print mid-line — follow mode only).
fn read_tail_from(path: &std::path::Path, pos: u64, len: u64, cap: u64) -> Res<(Vec<u8>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let start = pos.max(len.saturating_sub(cap));
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(len - start).read_to_end(&mut buf)?;
    Ok((buf, start))
}

/// Last `n` lines of bot.log (seek-capped at 256KB via
/// [`read_tail_from`]: the log grows to 8MB between rotations — never a
/// full-file read on a hot path).
pub(crate) fn tail_log(n: usize) -> Vec<String> {
    const CAP: u64 = 256 * 1024;
    let len = std::fs::metadata(proc::log_path())
        .map(|m| m.len())
        .unwrap_or(0);
    let Ok((bytes, _)) = read_tail_from(&proc::log_path(), 0, len, CAP) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(n);
    lines[skip..].iter().map(|l| l.to_string()).collect()
}

/// Log rotation: bot.log grows unbounded over long prod runs
/// (no rotation daemon watches it). Copy-truncate over 8MB into a
/// single backup — same inode, so writers appending across the call
/// never lose output. Called at boot and on the 60s watchdog tick
/// (size-check first, so the steady-state cost is one stat).
/// Best effort throughout: rotation must never fail the boot/tick.
pub(crate) fn rotate_log_if_huge() {
    const LIMIT: u64 = 8 << 20;
    let log = proc::log_path();
    let Ok(meta) = std::fs::metadata(&log) else {
        return;
    };
    if meta.len() <= LIMIT {
        return;
    }
    let bak = std::path::PathBuf::from("bot.log.1");
    if std::fs::copy(&log, &bak).is_ok()
        && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&log)
        && f.set_len(0).is_ok()
    {
        // Rotation copies the mode too: chmod the backup like open_log,
        // or a pre-existing 0644 keeps leaking chat IDs and excerpts.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&bak) {
                let mut perm = meta.permissions();
                perm.set_mode(0o600);
                let _ = std::fs::set_permissions(&bak, perm);
            }
        }
        println!("[main] rotated bot.log (was {} bytes)", meta.len());
    }
}

/// Foreground supervised run until the child exits or a signal lands.
/// No backgrounding: persistent production belongs to launchd. Stops
/// gracefully (TERM flushes the daemon offset) on Ctrl-C/TERM/HUP, and
/// reports a failed exit instead of silently returning Ok.
pub(crate) async fn run_foreground(home: &str, port: u16) -> Res<()> {
    if proc::port_busy(port) {
        say(
            home,
            &format!("port {port} busy — `dev cleanup` to clear, or stop the other owner"),
        );
        return Err("guard port busy".into());
    }
    say(home, "compiling…");
    let (lines, ok) = proc::cargo(&["build"], home).await;
    for l in lines {
        say(home, &l);
    }
    if !ok {
        return Err("build failed".into());
    }
    let mut child = proc::spawn_bot(&proc::log_path())?;
    say(
        home,
        &format!("bot running (PID: {:?}) — Ctrl-C stops", child.id()),
    );
    let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        super::shutdown_watch().await;
        let _ = stx.send(());
    });
    let exit = tokio::select! {
        _ = srx.recv() => {
            say(home, "signal — stopping…");
            None
        }
        st = child.wait() => Some(st),
    };
    proc::stop_graceful(&mut child).await;
    if let Some(Ok(st)) = exit
        && !st.success()
    {
        return Err(format!("bot exited: {st}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_tail_from_seeks_and_caps() {
        let dir = std::env::temp_dir().join(format!("herdr-tail-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.log");
        std::fs::write(&path, b"aaaa\nbbbb\ncccc\ndddd\n").unwrap();
        // Full window from 0.
        let (b, at) = read_tail_from(&path, 0, 20, 1024).unwrap();
        assert_eq!((at, &b[..]), (0, &b"aaaa\nbbbb\ncccc\ndddd\n"[..]));
        // Gap over cap jumps the cursor.
        let (b, at) = read_tail_from(&path, 0, 20, 6).unwrap();
        assert_eq!(at, 14);
        assert_eq!(&b[..], b"\ndddd\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
