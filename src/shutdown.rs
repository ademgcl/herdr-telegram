//! Shutdown signal: split from `main` (300-line file limit).
/// SIGINT, SIGTERM (launchd/docker send TERM), or SIGHUP (reload-style
/// stop): break the poll loop so the offset flushes instead of replaying
/// the batch on next boot.
/// Pending intents are already durable per-write; topics/focus likewise.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let term = signal(SignalKind::terminate());
        let hup = signal(SignalKind::hangup());
        match (term, hup) {
            (Ok(mut term), Ok(mut hup)) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = term.recv() => {},
                _ = hup.recv() => {},
            },
            (Ok(mut term), Err(_)) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = term.recv() => {},
            },
            (Err(_), Ok(mut hup)) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = hup.recv() => {},
            },
            (Err(_), Err(_)) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Signal-aware sleep: true when TERM arrived (caller saves offset and
/// exits), false on a full sleep. Single source for main's poll-backoff
/// waits — dup'd select blocks re-drift.
pub async fn sleep_or_shutdown(secs: u64) -> bool {
    tokio::select! {
        _ = shutdown_signal() => true,
        _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => false,
    }
}

/// Offset-advance verdict (pure, tested): a malformed update with no
/// `update_id` reads as id 0 — real Telegram update_ids are positive,
/// so 0 never advances the offset (poison guard). Single source for
/// main's ack-after-handling.
pub fn should_advance_offset(id: u64, off: u64) -> bool {
    id > 0 && id >= off
}

/// Offset after dropping a poison-only batch (pure, tested): id-0
/// updates never ack individually, so an all-poison `getUpdates` reply
/// would re-fetch forever (hot loop — Telegram returns immediately when
/// updates are pending). Step the cursor so the undeliverable batch is
/// not pinned at the same offset; from 0 reach 1 (past the poison).
pub fn poison_advance_offset(off: u64) -> u64 {
    if off == 0 { 1 } else { off + 1 }
}

/// Ack-after-handling (single source for main's poll loop): bump the
/// offset only past the poison guard above (at-least-once — bumping
/// before the handler acked a never-handled update on TERM is a silent
/// prompt loss; a malformed id 0 never advances, so it can't poison).
pub async fn ack_update(s: &crate::state::AppState, id: u64) {
    let mut off = s.offset.lock().await;
    if should_advance_offset(id, *off) {
        *off = id + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_advance_offset_poison_guard() {
        // Normal advance + idempotent re-ack.
        assert!(should_advance_offset(10, 10));
        assert!(should_advance_offset(11, 10));
        // Poison id 0 never advances, even from zero.
        assert!(!should_advance_offset(0, 0));
        assert!(!should_advance_offset(0, 10));
        // Old replays never move the offset backwards.
        assert!(!should_advance_offset(9, 10));
    }

    #[test]
    fn test_poison_advance_offset_makes_progress() {
        // All-poison batch: always step so the next poll is not the
        // same undeliverable window (hot-loop guard).
        assert_eq!(poison_advance_offset(0), 1);
        assert_eq!(poison_advance_offset(1), 2);
        assert_eq!(poison_advance_offset(41), 42);
    }
}
