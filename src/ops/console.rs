//! Foreground supervised console (dev.sh `dev` parity, line-based):
//! build, run the bot supervised, watch sources for rebuild+restart,
//! crash backoff, and a letter+Enter REPL. Background daemonization is
//! launchd's job — this never orphans.
use super::proc;
use crate::types::Res;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

/// mtime+len+name checksum over watched sources (std metadata, no
/// deps). Sorted: `read_dir` order is unspecified, and hashing in walk
/// order would flap rebuilds on nothing.
fn checksum() -> u64 {
    let mut files: Vec<(std::path::PathBuf, u64, u64)> = Vec::new();
    let mut walk = vec![std::path::PathBuf::from("src")];
    walk.push(std::path::PathBuf::from("Cargo.toml"));
    walk.push(std::path::PathBuf::from("Cargo.lock"));
    while let Some(p) = walk.pop() {
        let Ok(md) = std::fs::metadata(&p) else {
            continue;
        };
        if md.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    walk.push(e.path());
                }
            }
            continue;
        }
        let mtime = md
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        files.push((p, mtime, md.len()));
    }
    files.sort();
    let mut h = DefaultHasher::new();
    files.hash(&mut h);
    h.finish()
}

struct Console {
    home: String,
    port: u16,
    child: Option<tokio::process::Child>,
    last_start: Instant,
    crashes: u32,
    paused: bool,
    prev_sum: u64,
}

impl Console {
    async fn start(&mut self) {
        if self.child.is_some() {
            return;
        }
        if proc::port_busy(self.port) {
            self.say(&format!(
                "port {} busy — `cleanup` to clear, then `r`",
                self.port
            ));
            self.paused = true;
            return;
        }
        self.say("compiling…");
        let (lines, ok) = proc::cargo(&["build"], &self.home).await;
        for l in lines {
            self.say(&l);
        }
        if !ok {
            self.say("build FAILED — fix the code, auto-retry on change");
            return;
        }
        match proc::spawn_bot(&proc::log_path()) {
            Ok(child) => {
                self.say(&format!("bot started (PID: {:?})", child.id()));
                self.child = Some(child);
                self.last_start = Instant::now();
            }
            Err(e) => self.say(&format!("start failed: {e}")),
        }
    }

    async fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            proc::stop_graceful(&mut child).await;
        }
        self.child = None;
    }

    /// Health tick: reap exited child, apply crash/backoff policy.
    async fn health(&mut self) {
        let exited = match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(Some(_))),
            None => false,
        };
        if !exited {
            return;
        }
        self.child = None;
        if self.last_start.elapsed() < std::time::Duration::from_secs(4) {
            self.crashes += 1;
            self.say("bot exited immediately — recent log:");
            for l in proc::tail_log(8) {
                self.say(&l);
            }
            if self.crashes >= 3 {
                self.say("3 consecutive crashes — paused. `r` retries, `x` clears conflicts");
                self.paused = true;
                return;
            }
        } else {
            self.crashes = 0;
            self.say("bot stopped.");
        }
        if !self.paused {
            self.say("restarting…");
            self.start().await;
        }
    }

    fn say(&self, line: &str) {
        println!("{}", super::mask::mask_line(line, &self.home));
    }

    /// Single-stdin prompt: the console owns the only stdin reader (its
    /// channel) — opening a second reader here would race it for input
    /// lines and appear hung. Empty/EOF cancels.
    async fn ask(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
        msg: &str,
    ) -> String {
        use std::io::Write;
        print!("{msg}");
        let _ = std::io::stdout().flush();
        rx.recv().await.unwrap_or_default().trim().to_string()
    }

    async fn key(
        &mut self,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
        line: &str,
    ) -> bool {
        let cmd = line.split_whitespace().next().unwrap_or("");
        match cmd {
            "r" | "restart" => {
                self.say("restarting…");
                self.paused = false;
                self.crashes = 0;
                self.stop_child().await;
                self.start().await;
                self.prev_sum = checksum();
            }
            "b" | "build" => {
                self.say("cargo check…");
                let (lines, _) = proc::cargo(&["check"], &self.home).await;
                for l in lines {
                    self.say(&l);
                }
            }
            "t" | "topics" => {
                super::ctl_topics(&self.home).await;
                let pane = Self::ask(rx, "pane/topic to reset (empty = cancel): ").await;
                if !pane.is_empty() {
                    super::ctl_reset(&pane).await;
                }
            }
            "e" | "event" => {
                let pane = Self::ask(rx, "pane (e.g. w1:p2, empty = cancel): ").await;
                if pane.is_empty() {
                    return true;
                }
                let st = Self::ask(rx, "status [blocked|working|done|idle]: ").await;
                if !st.is_empty() {
                    super::ctl_trigger(&pane, &st).await;
                }
            }
            "l" | "logs" | "log" => {
                for l in proc::tail_log(25) {
                    self.say(&l);
                }
                if Self::ask(rx, "follow live? [y/N]: ").await.to_lowercase().starts_with('y') {
                    self.say("following — `q` + Enter exits follow");
                    super::cmd::follow_log(&self.home, rx).await;
                }
            }
            "s" | "status" => super::cmd::print_status(self.port),
            "c" | "clear" => match crate::types::write_private(&proc::log_path(), b"") {
                Ok(()) => self.say("bot.log cleared."),
                Err(e) => self.say(&format!("clear failed: {e}")),
            },
            "k" | "stop" => {
                self.say("bot stopped.");
                self.stop_child().await;
                self.paused = true;
            }
            "x" | "cleanup" => {
                self.stop_child().await;
                let stopped = proc::stop_all(self.port).await;
                self.say(&format!("cleared {} process(es)", stopped.len()));
                self.paused = false;
                self.crashes = 0;
                self.start().await;
                self.prev_sum = checksum();
            }
            "h" | "help" | "?" => super::print_console_help(),
            "q" | "quit" => {
                self.say("shutting down…");
                self.stop_child().await;
                return false;
            }
            "" => {}
            _ => self.say("unknown key — `h` for help"),
        }
        super::print_quick_menu();
        true
    }
}

/// `dev` with no subcommand: foreground supervised console. Line-based
/// REPL (letter + Enter): raw single-key mode needs a termios dep.
pub async fn console() -> Res<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let port = proc::guard_port();
    println!("herdr-telegram dev console (letter + Enter; `h` for help)");
    let mut con = Console {
        home,
        port,
        child: None,
        last_start: Instant::now(),
        crashes: 0,
        paused: false,
        prev_sum: 0,
    };
    con.start().await;
    con.prev_sum = checksum();
    super::print_quick_menu();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (stx, mut srx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        super::shutdown_watch().await;
        let _ = stx.send(());
    });
    loop {
        tokio::select! {
            _ = srx.recv() => {
                con.say("signal — shutting down…");
                con.stop_child().await;
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                con.health().await;
                let sum = checksum();
                if sum != con.prev_sum {
                    con.prev_sum = sum;
                    println!("change detected, rebuilding…");
                    con.paused = false;
                    con.crashes = 0;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    con.stop_child().await;
                    con.start().await;
                    con.prev_sum = checksum();
                }
            }
            line = rx.recv() => {
                let Some(line) = line else {
                    // EOF (piped input ends, SSH drop): stop the child —
                    // `kill_on_drop` alone may not run on teardown.
                    con.stop_child().await;
                    break;
                };
                if !con.key(&mut rx, &line).await {
                    break;
                }
            }
        }
    }
    Ok(())
}
