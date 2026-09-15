mod config;
mod handlers;
mod herdr;
mod jobs;
mod notifier;
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
    state::State,
    telegram::{get_updates, handle_update},
    types::{HERDR_PROTOCOL, Res, SINGLE_INSTANCE_PORT, TG_POLL_SECS},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Res<()> {
    // Single-instance guard: prevent duplicate instances from doubling notifications
    let lock_addr = format!("127.0.0.1:{SINGLE_INSTANCE_PORT}");
    let _guard = tokio::net::TcpListener::bind(&lock_addr)
        .await
        .map_err(|_| "another herdr-telegram instance is already running")?;

    let cfg = cfg_from_env()?;
    let s = State::new(cfg)?;

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
                ping_err = e.to_string();
                if attempt == 5 {
                    break;
                }
                eprintln!(
                    "[herdr] ping failed (attempt {attempt}/5): {ping_err} — retrying in 10s"
                );
                tokio::time::sleep(Duration::from_secs(10)).await;
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

    if let Some(forum_id) = s.cfg.forum {
        println!("[telegram] forum supergroup enabled: chat_id = {forum_id}");
    } else {
        println!("[telegram] operating in direct message mode");
    }

    // Register Telegram menu commands
    let _ = s.tg.set_my_commands().await;

    // Seed agent status without emitting alert noise
    reconcile(&s, true, "seed").await;
    // Re-arm prompt watchers orphaned by a restart (replies would else be lost)
    recover_pending(&s).await;
    // One-time cleanup of the retired pinned-status era (no-op when clean)
    s.topics.retire_pins().await;
    println!("[main] seed done, entering loop");

    // NOTE: no backlog discard — messages sent while the bot was down are
    // still delivered; router's stale filter (>10 min) drops only true relics.

    // Spawn background Herdr event stream task
    tokio::spawn(event_task(s.clone()));

    let mut watchdog_tick = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                println!("[main] shutdown signal — saving offset");
                s.save_offset().await;
                break;
            }
            _ = watchdog_tick.tick() => {
                reconcile(&s, false, "watchdog").await;
            }
            updates = async {
                let off = *s.offset.lock().await;
                get_updates(&s.tg, off, TG_POLL_SECS).await
            } => {
                match updates {
                    Ok(list) => {
                        if !list.is_empty() {
                            println!("[tg] poll ok: {} update(s)", list.len());
                        }
                        for u in list {
                            let id = u["update_id"].as_u64().unwrap_or(0);
                            {
                                let mut off = s.offset.lock().await;
                                if id >= *off {
                                    *off = id + 1;
                                }
                            }
                            handle_update(s.clone(), &u).await;
                        }
                        // Durable ack per batch: a crash before the next
                        // poll must not replay these prompts.
                        s.save_offset().await;
                    }
                    Err(e) => {
                        let msg = s.tg.redact(&e.to_string());
                        if msg.contains("Conflict") {
                            eprintln!("[tg] poll conflict (overlap), backing off 30s: {msg}");
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        } else {
                            eprintln!("[tg] poll failed: {msg}");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// SIGINT or SIGTERM (launchd/docker send TERM): break the poll loop so
/// the offset flushes instead of replaying the batch on next boot.
/// Pending intents are already durable per-write; topics/focus likewise.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = term.recv() => {},
            },
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
