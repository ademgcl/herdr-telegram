//! Core types for limit/stall episodes. Leaf module, std-only.

pub struct LimitHit {
    /// Stable per-episode signature: `rate-limit` | `auth` | `provider` | `error`.
    /// Dedup keys on this (NOT the excerpt — countdowns like
    /// `retrying in 12s` change every second and must not re-alert).
    pub kind: &'static str,
    /// The matched screen line (trimmed, capped) for the card body.
    pub excerpt: String,
    /// True when the match came from the STRONG table (specific banner
    /// wording, stands alone). WEAK matches fired on common words plus
    /// screen-wide context — agent prose or log dumps can trip them
    /// transiently, so WEAK `auth` must prove persistence (stuck gate)
    /// while STRONG `auth` (`permission_denied`…) pages at once.
    pub strong: bool,
}

/// Episode kind for fatal provider request failures: buzzed mid-run
/// only when stuck (see episode.rs), surfaced at settle otherwise.
pub const ERROR_KIND: &str = "error";

/// Kinds that buzz mid-run only when stuck (transient blips recover on
/// their own — e.g. `Provider response headers timed out … [retrying
/// attempt #N]` succeeds on attempt #2). `rate-limit`/STRONG-`auth`
/// still buzz immediately: quota stalls never self-heal and specific
/// auth denials (`permission_denied`…) persist until the owner fixes
/// login. WEAK `auth` (common words like `session expired` in prose or
/// log dumps) is stuck-gated too — see [`needs_stuck_gate`].
pub fn is_stuck_gated(kind: &str) -> bool {
    kind == ERROR_KIND || kind == "provider"
}

/// Per-hit buzz gate: kind-gated kinds plus WEAK `auth`. A WEAK auth
/// match is a common word plus ambient context — it must persist like
/// any other transient before paging; a STRONG one names the denial.
pub fn needs_stuck_gate(hit: &LimitHit) -> bool {
    is_stuck_gated(hit.kind) || (hit.kind == "auth" && !hit.strong)
}
