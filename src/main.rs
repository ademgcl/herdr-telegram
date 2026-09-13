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

use std::time::Duration;
use crate::{
    config::cfg_from_env,
    herdr::{event_task, ping},
    notifier::reconcile,
    state::State,
    telegram::{get_updates, handle_update},
    types::{Res, HERDR_PROTOCOL, SINGLE_INSTANCE_PORT, TG_POLL_SECS},
};

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
    let pong = ping(&s.cfg.socket).await?;
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
            _ = watchdog_tick.tick() => {
                reconcile(&s, false, "watchdog").await;
            }
            updates = async {
                let off = *s.offset.lock().await;
                get_updates(&s.tg, off, TG_POLL_SECS).await
            } => {
                match updates {
                    Ok(list) => {
                        println!("[tg] poll ok: {} update(s)", list.len());
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
                    }
                    Err(e) => {
                        eprintln!("[tg] poll failed: {e}");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
}
