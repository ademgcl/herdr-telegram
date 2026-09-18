//! Shutdown signal: split from `main` (300-line file limit).
/// SIGINT or SIGTERM (launchd/docker send TERM): break the poll loop so
/// the offset flushes instead of replaying the batch on next boot.
/// Pending intents are already durable per-write; topics/focus likewise.
pub async fn shutdown_signal() {
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
