//! herdr-telegram: Telegram (DMs + forum topics) ↔ Herdr multiplexer
//! over local Unix socket RPC. Init + poll/watchdog loop live here.
mod config;
mod ctl;
mod ctl_auth;
mod ctl_cmd;
mod ctl_inspect;
mod handlers;
mod herdr;
mod jobs;
mod notifier;
mod ops;
mod shutdown;
mod state;
mod telegram;
mod topics;
mod types;
mod ui;

use crate::{
    config::cfg_from_env,
    herdr::{event_task, ping},
    jobs::recover_pending,
    notifier::reconcile,
    shutdown::{shutdown_signal, sleep_or_shutdown},
    state::State,
    telegram::{get_updates, handle_update},
    types::{HERDR_PROTOCOL, Res, TG_POLL_SECS, home_masked},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    // .env first: dev.sh saw the file's port for the guard bind, so the
    // binary must too (otherwise a .env-only HERDR_TG_PORT is ignored).
    // Single source: `ops::proc::guard_port` (invalid fails loudly here
    // and in every ops call site — never a silent split-brain fallback).
    let port: u16 = crate::ops::proc::guard_port()?;

    // CLI control client mode: bypass daemon bind and talk to running bot
    if args.len() > 1 && args[1] == "ctl" {
        return ctl::run_ctl_client(port, &args[2..]).await;
    }

    // Local ops console (replaces dev.sh): supervised runs, status,
    // logs, cleanup, build, and ctl passthrough. Never binds the guard.
    if args.len() > 1 && (args[1] == "dev" || args[1] == "ops") {
        return ops::run(&args[2..]).await;
    }

    let lock_addr = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&lock_addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return Err("another herdr-telegram instance is already running".into());
        }
        Err(e) => return Err(format!("single-instance guard bind failed: {e}").into()),
    };

    let cfg = cfg_from_env()?;
    // Never print the home dir (username) or numeric chat IDs: bot.log is
    // a local diagnostic file, not a place for identifiers.
    println!("[main] state dir: {}", home_masked(&state::state_dir()));
    crate::ops::rotate_log_if_huge();
    let s = State::new(cfg)?;
    tokio::spawn(ctl::run_control_server(s.clone(), listener));

    // Verify Herdr connectivity and protocol
    let mut pong = None;
    let mut ping_err = String::new();
    for attempt in 1..=5 {
        match ping(&s.cfg.socket).await {
            Ok(p) => {
                pong = Some(p);
                break;
            }
            Err(e) => {
                ping_err = crate::types::mask_home(&e.to_string());
                if attempt == 5 {
                    break;
                }
                eprintln!(
                    "[herdr] ping failed (attempt {attempt}/5): {ping_err} — retrying in 10s"
                );
                // Signal-aware: a deaf 40s boot stall starves TERM.
                tokio::select! {
                    _ = shutdown_signal() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
            }
        }
    }
    let Some(pong) = pong else {
        return Err(format!("herdr ping failed after 5 attempts: {ping_err}").into());
    };
    println!(
        "[herdr] server v{}, protocol {} (bot built against protocol {HERDR_PROTOCOL})",
        pong["version"], pong["protocol"]
    );
    if pong["protocol"].as_u64() != Some(HERDR_PROTOCOL) {
        eprintln!("[herdr] WARNING: unexpected protocol version — commands may fail");
    }

    if s.cfg.forum.is_some() {
        println!("[telegram] forum supergroup mode enabled (chat id masked)");
    } else {
        println!("[telegram] operating in direct message mode");
    }

    // Register Telegram menu commands without failing the boot: the
    // menu persists server-side once set, so an outage at boot must not
    // crash-loop the process under launchd — converge in background.
    s.tg.clone().spawn_menu_sync();

    // Re-arm prompt watchers orphaned by a restart FIRST (replies would
    // else be lost), then seed agent status without alert noise. Order
    // matters: the seed retire paths (dead-pane silent close,
    // agent→shell quit notice) consume owed intents — running them
    // before recover would wipe replies recover was about to save, and
    // an empty in-memory status would misclassify live shells as fresh
    // flips (ghost quit card). Recovered watchers racing the seed lose
    // deterministically to its last-writer-wins retires.
    if let Some(forum_id) = s.cfg.forum {
        // F9: probe bot permissions in forum supergroup
        match s.tg.check_forum_permissions(forum_id).await {
            Ok(perms) => {
                if !perms.is_admin {
                    eprintln!("[telegram] WARNING: bot is NOT an admin in the forum!");
                } else {
                    println!(
                        "[telegram] bot permissions: manage_topics={}, delete_messages={}",
                        perms.can_manage_topics, perms.can_delete_messages
                    );
                    if !perms.can_manage_topics {
                        eprintln!("[telegram] WARNING: bot lacks 'can_manage_topics' admin right!");
                    }
                }
            }
            Err(e) => eprintln!(
                "[telegram] permission probe failed: {}",
                s.tg.redact(&e.to_string())
            ),
        }

        // F4: verify custom emoji topic icon stickers
        match s.tg.get_forum_topic_icon_stickers().await {
            Ok(stickers) => {
                let missing = topics::names::check_context_icons(&stickers);
                if missing.is_empty() {
                    println!(
                        "[telegram] forum icon stickers verified ({} available; kind glyphs valid)",
                        stickers.len()
                    );
                } else {
                    eprintln!("[telegram] WARNING: kind icon stickers missing in set: {missing:?}");
                }
            }
            Err(e) => eprintln!(
                "[telegram] forum icon stickers probe failed: {}",
                s.tg.redact(&e.to_string())
            ),
        }
    }
    recover_pending(&s).await;
    // Seed agent status without emitting alert noise
    reconcile(&s, true, "seed").await;
    println!("[main] seed done, entering loop");

    // NOTE: no backlog discard — messages sent while the bot was down are
    // still delivered; router's stale filter (>10 min) drops only true relics.

    // Spawn background Herdr event stream task
    tokio::spawn(event_task(s.clone()));

    let mut watchdog_tick = tokio::time::interval(Duration::from_secs(60));
    // Skip catch-up bursts: a slow reconcile (many panes × RPCs) overrunning
    // 60s must resume with ONE tick, never back-to-back scans (double-buzz,
    // 429 storm). The loop awaits each reconcile inline, so ticks can't
    // overlap each other — only the queued backlog needs dropping. Overlap
    // with the event-task resubscribe scan is stopped inside `reconcile`
    // itself (process-wide single-flight; the loser skips).
    watchdog_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first interval tick fires immediately: consume it so boot isn't
    // a double-scan (seed reconcile just ran above).
    watchdog_tick.tick().await;

    // Consecutive poll failures for capped backoff (instant failures
    // like DNS-down must not hot-loop the log every 5s all outage).
    let mut poll_fails: u32 = 0;
    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                println!("[main] shutdown signal — saving offset");
                s.save_offset().await;
                break;
            }
            _ = watchdog_tick.tick() => {
                crate::ops::rotate_log_if_huge();
                // Tick bound: a sick-herdr/large-fleet scan must not stretch
                // the ≤60s herdr→tg guarantee into minutes — partial tick +
                // retry next cycle (reconcile is per-pane idempotent; Skip
                // alone prevents overlap, never lateness). Shutdown-aware:
                // TERM mid-scan saves the offset and exits instead of
                // stalling exit (and the offset flush) up to 50s.
                tokio::select! {
                    _ = shutdown_signal() => {
                        println!("[main] shutdown signal mid-watchdog — saving offset");
                        s.save_offset().await;
                        return Ok(());
                    }
                    _ = tokio::time::timeout(
                        Duration::from_secs(50),
                        reconcile(&s, false, "watchdog"),
                    ) => {}
                }
            }
            updates = async {
                let off = *s.offset.lock().await;
                get_updates(&s.tg, off, TG_POLL_SECS).await
            } => {
                match updates {
                    Ok(list) => {
                        poll_fails = 0;
                        if !list.is_empty() {
                            println!("[tg] poll ok: {} update(s)", list.len());
                        }
                        for u in list {
                            let id = u["update_id"].as_u64().unwrap_or(0);
                            // Poison id 0 (no update_id) would re-submit
                            // every poll forever — drop before handling.
                            if id == 0 {
                                eprintln!("[tg] dropping update with no update_id");
                                continue;
                            }
                            // Shutdown-aware: a long handler (spawn) must
                            // not starve TERM into a SIGKILL + replay.
                            tokio::select! {
                                _ = shutdown_signal() => {
                                    println!("[main] shutdown signal mid-batch — saving offset");
                                    s.save_offset().await;
                                    return Ok(());
                                }
                                _ = handle_update(s.clone(), &u) => {}
                            }
                            // Ack AFTER handling (single source:
                            // [`crate::shutdown::ack_update`]).
                            crate::shutdown::ack_update(&s, id).await;
                            // Durable ack per update (at-least-once otherwise:
                            // a mid-batch crash would replay handled prompts
                            // as duplicate submits).
                            s.save_offset().await;
                        }
                    }
                    Err(e) => {
                        poll_fails = poll_fails.saturating_add(1);
                        let msg = s.tg.redact(&e.to_string());
                        // Backoff sleeps stay signal-aware: a deaf 30s
                        // sleep starves TERM into SIGKILL + replay.
                        // Revoked-token 401/404 never recovers by
                        // retrying: FATAL-break like menu sync so a fixed
                        // replacement can bind instead of squatting the
                        // guard forever.
                        if crate::telegram::client::TelegramClient::is_unauthorized(&msg) {
                            eprintln!("[tg] FATAL: getUpdates unauthorized/not-found (401/404) — token revoked?");
                            s.save_offset().await;
                            break;
                        }
                        if msg.contains("Conflict") {
                            eprintln!("[tg] poll conflict (overlap), backing off 30s: {msg}");
                            if sleep_or_shutdown(30).await {
                                println!("[main] shutdown signal during backoff — saving offset");
                                s.save_offset().await;
                                return Ok(());
                            }
                        } else {
                            // Flood-aware: a 429 `retry after N` overrides
                            // the capped backoff — re-hitting early only
                            // extends the flood (see flood_aware_poll_wait).
                            let wait = crate::telegram::polling::flood_aware_poll_wait(
                                &msg, poll_fails,
                            );
                            eprintln!("[tg] poll failed (retry in {wait}s): {msg}");
                            if sleep_or_shutdown(wait).await {
                                println!("[main] shutdown signal during backoff — saving offset");
                                s.save_offset().await;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
