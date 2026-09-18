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
