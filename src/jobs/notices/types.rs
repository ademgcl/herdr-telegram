//! Core types for limit/stall episodes. Leaf module, std-only.

pub struct LimitHit {
    /// Stable per-episode signature: `rate-limit` | `auth` | `provider` | `error`.
    /// Dedup keys on this (NOT the excerpt — countdowns like
    /// `retrying in 12s` change every second and must not re-alert).
    pub kind: &'static str,
    /// The matched screen line (trimmed, capped) for the card body.
    pub excerpt: String,
}

/// Episode kind for fatal provider request failures: buzzed mid-run
/// only when stuck (see episode.rs), surfaced at settle otherwise.
pub const ERROR_KIND: &str = "error";

/// Kinds that buzz mid-run only when stuck (transient blips recover on
/// their own — e.g. `Provider response headers timed out … [retrying
/// attempt #N]` succeeds on attempt #2). `rate-limit`/`auth` still buzz
/// immediately: quota/auth stalls never self-heal.
pub fn is_stuck_gated(kind: &str) -> bool {
    kind == ERROR_KIND || kind == "provider"
}
