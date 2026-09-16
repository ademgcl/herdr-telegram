/// Rate-limit / quota / provider-error detection on raw pane screens.
///
/// Why this exists: when opencode exhausts free usage it prints
/// `Free usage exceeded, subscribe to Go [retrying in …]` and retries
/// internally. herdr keeps reporting `working` — no status transition —
/// so neither the prompt watcher (waits for settle) nor the notifier
/// (fires on blocked/done/idle) says a word, and Telegram live-edits
/// don't buzz. The run stalls silently for minutes/hours.
///
/// Both the prompt watcher (`runner`) and the watchdog (`reconcile`)
/// scan screens with [`detect_limit`] and post one buzzing card per
/// episode so the owner notices.
pub mod card;
pub mod detect;
pub mod patterns;
pub mod types;
pub use card::limit_card_text;
pub use detect::{detect_limit, is_provider_failure_line, screen_has_provider_failure};
pub use types::{LimitHit, needs_stuck_gate};
