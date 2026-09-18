pub mod emoji;
pub mod keyboards;
pub mod scope_text;
pub mod views;

pub use emoji::*;
pub use keyboards::*;
pub use views::*;

/// Single source for the fail-closed outage ack (ambiguous herdr read →
/// no write, visible retry). Dup'd literals re-drift — use this.
pub const HERDR_UNREACHABLE: &str = "⚠️ herdr unreachable — try again";

/// Single source for the unknown-target ack (explicit arg naming nothing
/// live must never answer for a different agent). 11 sites share it.
pub const UNKNOWN_TARGET: &str = "unknown target — see /agents";

/// Single source for the tap contention ack (a tap/type already owns the
/// pane — the retry is just sending again in a beat).
pub const ANSWER_IN_FLIGHT: &str = "answer already in flight — wait a beat";

/// Single source for the keys contention ack (tap/model owns pane input).
pub const TAP_MODEL_IN_FLIGHT: &str = "tap/model op in flight — wait a beat";

/// Single source for the /esc contention ack (an answer owns the pane).
pub const ESC_IN_FLIGHT: &str = "answer in flight — wait a beat, then /esc";

/// Single source for the failed-submit-blocked ack when the question card
/// is already up (or owned in flight): plain text, no second ❗ buzz.
pub const BLOCKED_SEE_CARD: &str = "⛔ blocked — see question card";

/// Single source for the /card contention ack (an answer owns the pane).
pub const CARD_IN_FLIGHT: &str = "answer in flight — wait a beat, then /card";

/// Single source for the keys-sent ack shared by every keys arm.
pub const KEYS_SENT: &str = "⌨️ keys sent";

/// Single source for the unreadable-space ack (fail-closed: never open in
/// the wrong space).
pub const SPACE_UNREADABLE: &str = "⚠️ could not read space — try again";

/// Single source for the empty-output fallback shared by every output
/// surface (pane/agent reads, shell results, topic reads).
pub const NO_OUTPUT: &str = "(no output)";

/// Single source for the typed-answer ack (every typed-answer surface).
pub fn typed_ack(pane: &str) -> String {
    format!("⌨️ typed into {pane} + ⏎")
}

/// Single source for the stranded-typewait shell refuse (typed answer
/// landing across an agent→shell flip: fail-closed, never run as shell).
pub const STALE_TYPEWAIT_SHELL: &str =
    "that answer was armed for a question, but this is now a shell — re-send as a shell command if intended";

/// Single source for the card-failure fallback (failed type + failed
/// repost: answer on the PC). Dup'd literals re-drift — use this.
pub const CARD_FAILED_PC: &str = "card failed too — answer on the PC";
