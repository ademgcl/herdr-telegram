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
        offset: std::fs::read_to_string(dir.join("offset.state")).ok().map(|s| s.trim().to_string()),
        topic_titles: std::fs::read_to_string(dir.join("topics.state"))
            .map(|t| t.lines().filter(|l| l.contains("\":")).count())
            .unwrap_or(0),
        log_bytes: std::fs::metadata("bot.log").map(|m| m.len()).unwrap_or(0),
    }
}

pub(crate) fn print_status(port: u16) {
    let home = std::env::var("HOME").unwrap_or_default();
    let st = snapshot(port);
    say(&home, "=== HERDR TELEGRAM STATUS ===");
    if st.bot_pids.is_empty() {
        say(&home, "  Bot Status:   Stopped");
    } else {
        say(
            &home,
            &format!("  Bot Status:   Running (PID: {})", st.bot_pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")),
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
            if st.socket_ok { "Available" } else { "Not found" },
            proc::herdr_socket()
        ),
    );
    if let Some(off) = st.offset {
        say(&home, &format!("  Offset State: {off}"));
    }
    say(&home, &format!("  Topics State: ~{} titles", st.topic_titles));
    say(&home, &format!("  Log File:     bot.log ({} bytes)", st.log_bytes));
    say(&home, "=============================");
}

/// Follow bot.log from its current end (callers print history first).
/// Shares the caller's stdin channel — a second reader would race it
/// for `q` lines. Partial trailing lines held for the next poll.
pub(crate) async fn follow_log(home: &str, rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) {
    println!("following bot.log — `q` + Enter exits (masking on)");
    let mut pos = std::fs::metadata(proc::log_path())
        .map(|m| m.len())
        .unwrap_or(0);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                let len = std::fs::metadata(proc::log_path()).map(|m| m.len()).unwrap_or(pos);
                if len > pos
                    && let Ok(bytes) = std::fs::read(proc::log_path())
                    && bytes.len() as u64 >= pos
                {
                    let new = &bytes[pos as usize..];
                    // Hold back a partial trailing line for next poll.
                    let end = new.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
                    if end > 0 {
                        print!("{}", mask::mask_line(&String::from_utf8_lossy(&new[..end]), home));
                        pos += end as u64;
                    }
                } else {
                    pos = len.min(pos);
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

/// Foreground supervised run until the child exits or a signal lands.
/// No backgrounding: persistent production belongs to launchd. Stops
/// gracefully (TERM flushes the daemon offset) on Ctrl-C/TERM/HUP, and
/// reports a failed exit instead of silently returning Ok.
pub(crate) async fn run_foreground(home: &str, port: u16) -> Res<()> {
    if proc::port_busy(port) {
        say(home, &format!("port {port} busy — `dev cleanup` to clear, or stop the other owner"));
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
    say(home, &format!("bot running (PID: {:?}) — Ctrl-C stops", child.id()));
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
