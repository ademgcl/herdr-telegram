//! Retry-guard for spontaneous pushes: single source for the
//! moved-on verdict (idle↔done collapse, missing means moved-on).

/// Moved-on verdict shared by every settle_check arm: `None` (no status)
/// counts as moved-on (fail-closed, never buzz into the unknown), and
/// idle↔done collapse matches the arm check.
pub(crate) fn moved_on(cur: Option<&str>, settled: &str) -> bool {
    match cur {
        None => true,
        Some(st) => {
            st != settled
                && !(matches!(settled, "idle" | "done") && matches!(st, "idle" | "done"))
        }
    }
}

/// Current status snapshot for the guard above.
pub(crate) async fn cur_status(s: &crate::state::AppState, pane: &str) -> Option<String> {
    s.status.lock().await.get(pane).cloned()
}

/// Post-sleep re-check (window = 15s inter-retry sleep): work starting
/// mid-sleep owns the pane now — moved-on stays silent.
pub(crate) async fn moved_on_now(
    s: &crate::state::AppState,
    pane: &str,
    settled: &str,
) -> bool {
    moved_on(cur_status(s, pane).await.as_deref(), settled)
}

#[cfg(test)]
#[path = "retry_guard_tests.rs"]
mod tests;
