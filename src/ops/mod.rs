//! Local ops console (replaces dev.sh — repo is 100% Rust): supervised
//! foreground runs, status, logs, cleanup, cargo passthrough, and `ctl`
//! client passthrough. Display is masked at print time; files untouched.
//!
//! Operating rule: ONE owner per (port, state-dir, token). Background
//! production belongs to launchd; these commands never daemonize and
//! never auto-kill another owner — `cleanup`/`stop` are explicit only.
mod cmd;
mod console;
mod launchd;
mod mask;
mod proc;

pub(crate) use cmd::rotate_log_if_huge;
pub(crate) use mask::mask_line as mask_display_line;

use crate::types::Res;

/// Print one display line, secrets masked.
pub(crate) fn say(home: &str, line: &str) {
    println!("{}", mask::mask_line(line, home));
}

fn print_quick_menu() {
    println!("[r]estart [b]uild [t]opics [e]vent [l]ogs [s]tatus [c]lear [k]stop [x]cleanup [h]elp [q]uit");
}

fn print_console_help() {
    println!("r: rebuild+restart | b: cargo check | t: topics+reset | e: mock event | l: logs | s: status | c: clear log | k: stop | x: clear conflicts+start | q: quit (letter + Enter)");
}

fn print_help() {
    println!(
        "Usage: herdr-telegram dev [COMMAND] [ARGS...]
  (no command)          : interactive supervised console
  status                : bot, guard port, socket, state table
  logs [-n N] [--follow]: bot.log tail (masked)
  start | stop | restart: foreground supervised run / explicit stop
  cleanup               : stop bot PIDs + free the guard port (explicit)
  install | uninstall   : write/remove launchd plist + bootstrap/bootout (prod)
  build | check         : cargo build / cargo check
  topics|reset|trigger|inspect: control-socket passthrough (see ctl)
  help                  : this help
Background production belongs to launchd; dev commands never daemonize."
    );
}

/// Run the `ctl` client as a captured child (`current_exe ctl …`),
/// returning masked output + success. Display stays masked like every
/// other ops surface (dev.sh piped `cmd_ctl` through `mask`); the `ctl`
/// path itself is untouched. Env/cwd inherit, so port + token resolve
/// exactly as a direct `ctl` call.
async fn ctl_capture(args: &[String], home: &str) -> (Vec<String>, bool) {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("herdr-telegram"));
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("ctl").args(args);
    let Ok(out) = cmd.output().await else {
        return (vec!["ctl spawn failed".to_string()], false);
    };
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| mask::mask_line(l, home))
        .collect();
    if !out.status.success() {
        for l in String::from_utf8_lossy(&out.stderr).lines() {
            lines.push(mask::mask_line(l, home));
        }
    }
    (lines, out.status.success())
}

async fn ctl_topics(home: &str) {
    let (lines, _) = ctl_capture(&["topics".to_string()], home).await;
    for l in lines {
        say(home, &l);
    }
}

async fn ctl_reset(pane: &str) {
    let home = std::env::var("HOME").unwrap_or_default();
    let (lines, _) = ctl_capture(&["reset".to_string(), pane.to_string()], &home).await;
    for l in lines {
        say(&home, &l);
    }
}

async fn ctl_trigger(pane: &str, status: &str) {
    let home = std::env::var("HOME").unwrap_or_default();
    let (lines, _) = ctl_capture(
        &["trigger".to_string(), pane.to_string(), status.to_string()],
        &home,
    )
    .await;
    for l in lines {
        say(&home, &l);
    }
}

/// INT/TERM/HUP trap (dev.sh `trap cleanup_and_exit` parity): a signal
/// kills the process without running drops, so `kill_on_drop` alone
/// would orphan the supervised daemon behind the guard port. Shared by
/// the console loop and one-shot `start`.
pub(crate) async fn shutdown_watch() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        let term = tokio::signal::unix::signal(SignalKind::terminate()).ok();
        let hup = tokio::signal::unix::signal(SignalKind::hangup()).ok();
        match (term, hup) {
            (Some(mut t), Some(mut h)) => {
                tokio::select! {
                    _ = t.recv() => {}
                    _ = h.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            _ => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `dev|ops` entry: no subcommand opens the console; otherwise one shot.
pub async fn run(args: &[String]) -> Res<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let port = proc::guard_port();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "" | "watch" | "run" => console::console().await,
        "status" => {
            cmd::print_status(port);
            Ok(())
        }
        "logs" | "log" | "tail" => {
            let rest: Vec<&String> = args.iter().skip(1).collect();
            let mut n = 50usize;
            let mut follow = false;
            let mut i = 0;
            // Bare number (`logs 25`) means -n, like tail.
            if rest.first().is_some_and(|a| a.parse::<usize>().is_ok()) {
                n = rest[0].parse().unwrap_or(50);
                i = 1;
            }
            while i < rest.len() {
                if rest[i] == "-n" {
                    i += 1;
                    n = rest.get(i).and_then(|v| v.parse().ok()).unwrap_or(50);
                } else if rest[i] == "--follow" || rest[i] == "-f" {
                    follow = true;
                }
                i += 1;
            }
            if follow {
                // History first, like `tail -n N -f` (follow starts at EOF).
                let lines = proc::tail_log(n);
                if lines.is_empty() && std::fs::metadata(proc::log_path()).is_err() {
                    say(&home, "bot.log not created yet.");
                }
                for l in lines {
                    say(&home, &l);
                }
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lines().map_while(Result::ok) {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                });
                cmd::follow_log(&home, &mut rx).await;
            } else {
                let lines = proc::tail_log(n);
                if lines.is_empty() && std::fs::metadata(proc::log_path()).is_err() {
                    say(&home, "bot.log not created yet.");
                }
                for l in lines {
                    say(&home, &l);
                }
            }
            Ok(())
        }
        "cleanup" | "clean" => {
            let stopped = proc::stop_all(port).await;
            if proc::port_busy(port) {
                say(&home, &format!("stopped {} process(es) — port {port} STILL busy (foreign owner?)", stopped.len()));
            } else {
                say(&home, &format!("cleared {} process(es), port {port} released", stopped.len()));
            }
            Ok(())
        }
        "start" => cmd::run_foreground(&home, port).await,
        "stop" => {
            let stopped = proc::stop_all(port).await;
            say(&home, &format!("stopped {} process(es)", stopped.len()));
            Ok(())
        }
        "restart" => {
            let stopped = proc::stop_all(port).await;
            say(&home, &format!("stopped {} process(es)", stopped.len()));
            cmd::run_foreground(&home, port).await
        }
        "install" => launchd::install(&home).await,
        "uninstall" => launchd::uninstall(&home).await,
        "build" => {
            let (lines, ok) = proc::cargo(&["build"], &home).await;
            for l in lines {
                say(&home, &l);
            }
            if ok { Ok(()) } else { Err("build failed".into()) }
        }
        "check" => {
            let (lines, ok) = proc::cargo(&["check"], &home).await;
            for l in lines {
                say(&home, &l);
            }
            if ok { Ok(()) } else { Err("check failed".into()) }
        }
        "topics" | "reset" | "trigger" | "inspect" => {
            // `args` already excludes the `dev` word itself.
            let (lines, ok) = ctl_capture(args, &home).await;
            for l in lines {
                say(&home, &l);
            }
            if ok { Ok(()) } else { Err("ctl command failed".into()) }
        }
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        _ => {
            print_help();
            Err(format!("unknown dev command: {cmd}").into())
        }
    }
}
