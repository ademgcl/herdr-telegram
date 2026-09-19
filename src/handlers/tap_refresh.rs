use crate::state::AppState;
use std::time::Duration;

/// Delayed self-heal for non-card outcomes (typed answers do the same):
/// slow renders and lagging status resolve in seconds via refresh
/// instead of the ≤60s watchdog.
pub async fn delayed_refresh(s: &AppState, pane: &str) {
    let s2 = s.clone();
    let pane2 = pane.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        crate::handlers::dialog::refresh_blocked_card(&s2, &pane2).await;
    });
}
