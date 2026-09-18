//! Shell settle wait: poll the snapshot + the foreground-process busy
//! signal until the command completes. Split from `shell_common`
//! (300-line file limit).
use crate::{
    herdr::client::{is_shell_idle, read_shell_output},
    state::AppState,
};
use tokio::time::{Duration, sleep};

/// First-settle poll budget: 15 rounds of 1s sleeps (nominal ~15s;
/// wall clock stretches under sick-herdr RPCs — snapshot reads up to
/// 30s, the busy probe 10s — so the typing sustain below is time-based,
/// never per-iteration). Unsettled commands post their tail card and
/// continue in `settle_report_shell` follow-ups.
const SETTLE_ROUNDS: u32 = 15;

/// Read one shell snapshot (best effort). Lives here with its only
/// poll-loop consumer (re-exported via `shell_common` for the submit
/// paths) so the settle split has no two-way module dependency.
pub(crate) async fn shell_snapshot(s: &AppState, pane: &str) -> String {
    read_shell_output(&s.cfg.socket, pane, 60)
        .await
        .unwrap_or_default()
}

/// Wait for the shell to settle after submitting: poll until the screen
/// is stable AND the shell owns the foreground again (or the budget).
/// Screen stability alone false-settles slow commands: the typed echo
/// sits unchanged for seconds before output arrives, and identical
/// polls then post the bare echo as the final card while the command
/// still runs — the full reply never follows (the intent is cleared).
/// The process gate fixes it: a running command owns the foreground
/// group, an idle shell is back at its prompt.
/// Returns whether the screen stabilized: callers must say so when it
/// did not, never present partial output as final. Aborts early when the
/// pane's pending intent vanishes: /cancel then waits out at most one
/// sleep plus in-flight herdr reads (no hard bound — reads can take
/// 30s+, but the abort itself adds no extra budget). The early return
/// carries whatever was last read (empty on the first round); callers
/// gate on the intent anyway, so it always stays silent.
/// Typing rides a dedicated sustain task on the shared cadence (well
/// inside the ≈5s expiry, so DM shells — no typing task — and sick-herdr
/// rounds that stretch past the expiry never go dark). Spawned, never
/// awaited; aborted on every exit below.
pub(crate) async fn await_shell_settle(
    s: &AppState,
    pane: &str,
    before: &str,
    chat: i64,
    thread: Option<i64>,
) -> (String, bool) {
    let sustain = {
        let tg = s.tg.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(crate::state::TYPING_TICK_SECS)).await;
                tg.typing(chat, thread).await;
            }
        })
    };
    let mut last = String::new();
    let mut stable = 0u32;
    let mut cur = String::new();
    // Consecutive blank snapshots (a live shell always renders a prompt,
    // so blank is a failed read, never a signal — see `note_blank`).
    // A window that reads blank throughout is a dead pane: settle blank
    // like the historic path instead of burning follow-ups on a corpse.
    let mut failed = 0u32;
    for _ in 0..SETTLE_ROUNDS {
        sleep(Duration::from_secs(1)).await;
        // Prompt abort, not a settle verdict: the caller gates on the
        // intent anyway — this just skips dead sleeping.
        if !s.pending.lock().await.contains_key(pane) {
            sustain.abort();
            return (cur, false);
        }
        // Sequential, snapshot first: the idle sample must be
        // contemporaneous with (or newer than) the screen it gates — a
        // join! would pair a fresh idle with a stale slow screen and
        // risk settling a command that started mid-read.
        let snap = shell_snapshot(s, pane).await;
        let idle = is_shell_idle(&s.cfg.socket, pane).await;
        if snap.trim().is_empty() {
            let (next, dead) = note_blank(failed);
            failed = next;
            if dead {
                break;
            }
            continue;
        }
        failed = 0;
        cur = snap;
        let (next, done) = settle_poll(&cur, before, &last, stable, idle);
        stable = next;
        last = cur.clone();
        if done {
            sustain.abort();
            return (cur, true);
        }
    }
    sustain.abort();
    // Every round blank: the pane is gone — blank Final, not follow-ups.
    if failed >= SETTLE_ROUNDS {
        return (String::new(), true);
    }
    (cur, false)
}

/// Pure blank-sample accounting for the settle window: a blank snapshot
/// banks no stability — the old loop settled two blank reads as a blank
/// Final. Isolated blanks deliberately leave the banked streak untouched
/// (the screen did not move; only banking is gated), so one failed poll
/// cannot spam extra cards. Returns (failed_streak, dead_window).
pub(crate) fn note_blank(failed: u32) -> (u32, bool) {
    let next = failed + 1;
    (next, next >= SETTLE_ROUNDS)
}

/// Pure per-round settle decision: bank stability only on a changed and
/// steady screen. A busy screen resets the streak — an echo frozen
/// while the command runs must never vest into a verdict. Unknown busy
/// (servers without process_info) degrades to a longer timing-only bar,
/// never the instant one. Idle bar 2 settles on the third identical
/// read (≈3s); unknown bar 4 on the fifth (≈5s — the entry read banks
/// nothing). Returns (stable, settled).
pub(crate) fn settle_poll(
    cur: &str,
    before: &str,
    last: &str,
    stable: u32,
    idle: Option<bool>,
) -> (u32, bool) {
    if cur == before || cur != last {
        return (0, false);
    }
    let bar = match idle {
        Some(true) => 2,
        None => 4,
        Some(false) => return (0, false),
    };
    let next = stable + 1;
    (next, next >= bar)
}

#[cfg(test)]
#[path = "shell_settle_tests.rs"]
mod tests;
