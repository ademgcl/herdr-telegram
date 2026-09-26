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
/// attempt #N]` succeeds on attempt #2, and transient 429/auto-retry
/// flashes clear long before the gate). `rate-limit` joins the gate:
/// quota-looking flashes that self-heal stay silent; only a banner that
/// persists a full stuck window pages. STRONG-`auth` still buzzes
/// immediately: specific denials (`permission_denied`…) persist until
/// the owner fixes login. WEAK `auth` (common words like
/// `session expired` in prose or log dumps) is stuck-gated too — see
/// [`needs_stuck_gate`].
pub fn is_stuck_gated(kind: &str) -> bool {
    kind == ERROR_KIND || kind == "provider" || kind == "rate-limit"
}

/// Per-hit buzz gate: kind-gated kinds plus WEAK `auth`. A WEAK auth
/// match is a common word plus ambient context — it must persist like
/// any other transient before paging; a STRONG one names the denial.
pub fn needs_stuck_gate(hit: &LimitHit) -> bool {
    is_stuck_gated(hit.kind) || (hit.kind == "auth" && !hit.strong)
}

/// Kinds that never page mid-run: transient auto-retry noise the agent
/// recovers from on its own (overload flashes, `[retrying attempt #N]`
/// banners that succeed seconds later). Worse, agent prose discussing
/// upstream handling trips the WEAK `provider` match and persists on
/// screen past the stuck gate — paging it is pure noise. Single source:
/// both buzz paths (`jobs::episode::tick`, `notifier::limits`) consult
/// this, so a newly-silenced kind stays silent everywhere.
pub fn is_transient_silent(kind: &str) -> bool {
    kind == "provider"
}
