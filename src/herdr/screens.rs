//! Terminal screen snapshots over herdr socket RPC.
use super::agents::{read_agent_output, read_agent_visible};

/// Visible-viewport snapshot — works on blocked panes where the
/// `recent_unwrapped` source errors out.
pub async fn read_screen_visible(socket: &str, pane: &str, lines: u32) -> Vec<String> {
    read_agent_visible(socket, pane, lines)
        .await
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Adaptive read for live streaming: herdr rejects large captures on busy
/// alternate-screen TUIs (any supported agent) but allows small visible tails;
/// line-printing agents allow large reads. Use the biggest window available.
pub async fn read_screen_adaptive(socket: &str, pane: &str) -> Vec<String> {
    let big = read_agent_output(socket, pane, 200).await;
    match big {
        Ok(text) if !text.is_empty() => wrap(text),
        _ => {
            let tail = read_agent_visible(socket, pane, LIVE_TAIL_LINES).await;
            tail.map(wrap).unwrap_or_default()
        }
    }
}

fn wrap(text: String) -> Vec<String> {
    text.lines().map(|l| l.trim_end().to_string()).collect()
}

/// Visible-tail width for blocked panes. Kept equal to
/// `handlers::dialog::DIALOG_READ_LINES` (single source would be a
/// layer inversion): blocked-card stamp and sig-compare windows must
/// match or tall dialogs double-buzz.
pub const LIVE_TAIL_LINES: u32 = 60;

/// Wide-window read for limit/stall scans: quota banners scroll far above
/// the live tail during hours-long auto-retry loops that print every
/// second (500 lines ≈ minutes of spam). Try the biggest window first,
/// degrade gracefully on busy alternate-screen TUIs (the 60-line visible
/// fallback matches the dialog window there). Busy TUIs pay
/// up to 3 RPCs per scan (500 + 200 fail, 60 lands) — accepted: limit
/// scans run at most every 5s per prompt pane, 60s per idle pane.
pub async fn read_screen_for_limits(socket: &str, pane: &str) -> Vec<String> {
    // Total budget: a sick herdr must not stall the 5s watcher / 60s
    // watchdog per pane (30s per-RPC timeouts × 3 fallbacks ≈ 90s).
    // Timeout reads as outage (empty): state preserved, next tick retries.
    tokio::time::timeout(std::time::Duration::from_secs(45), read_wide(socket, pane))
        .await
        .unwrap_or_default()
}

async fn read_wide(socket: &str, pane: &str) -> Vec<String> {
    if let Ok(text) = read_agent_output(socket, pane, 500).await
        && !text.trim().is_empty()
    {
        return wrap(text);
    }
    if let Ok(text) = read_agent_output(socket, pane, 200).await
        && !text.trim().is_empty()
    {
        return wrap(text);
    }
    read_agent_visible(socket, pane, LIVE_TAIL_LINES)
        .await
        .map(wrap)
        .unwrap_or_default()
}
