//! Birth nudge for the reconcile loop.
//!
//! Discovery had a 60s floor: the watchdog tick was the only thing that
//! noticed a new pane, so a pane spawned from the herdr CLI waited up to
//! a minute for a topic to exist — the reported "I thought it did not
//! arrive". herdr emits `pane.created` / `pane.agent_detected` for a
//! birth, so the event stream wakes the loop instead.
//!
//! Process-global, and that is correct rather than lazy: there is exactly
//! one reconcile loop per bot process, and one watchdog behind it.
//!
//! `Notify` over a flag on purpose — births can land while a scan is
//! running, and the loop must still wake for those. One permit is
//! retained, so a burst costs ONE extra scan instead of losing the tail.
use std::sync::LazyLock;
use tokio::sync::Notify;

static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Nudge the reconcile loop (birth seen).
pub fn wake() {
    WAKE.notify_one();
}

/// Await the next birth. Cancelled safely: a dropped future loses only
/// its permit claim, never a wake, because `Notify` keeps one stored.
pub async fn wait() {
    WAKE.notified().await;
}
