//! Boot sequence: herdr ping, mode banner, menu sync, forum probes.
//! Split from `main` (300-line file limit).
use crate::{
    herdr::ping,
    shutdown::shutdown_signal,
    state::State,
    topics,
    types::{HERDR_PROTOCOL, Res},
};
use std::time::Duration;

pub(crate) async fn boot(s: &State) -> Res<()> {
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
    Ok(())
}
