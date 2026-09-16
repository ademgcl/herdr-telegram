//! Match tables for limit/stall detection. Pure data, std-only.
use super::types::ERROR_KIND;

/// Strong patterns: specific enough to alert on their own. All
/// lowercase, matched as substrings against the lowercased line.
pub(crate) const STRONG: &[(&str, &str)] = &[
    ("free usage exceeded", "rate-limit"),
    ("usage exceeded", "rate-limit"),
    ("limit exceeded", "rate-limit"),
    ("rate limit exceeded", "rate-limit"),
    ("too many requests", "rate-limit"),
    ("quota exceeded", "rate-limit"),
    ("quota exhausted", "rate-limit"),
    // Google gRPC shape uses underscores (RESOURCE_EXHAUSTED) — match
    // both spellings; lines are lowercased, never re-spaced.
    ("resource exhausted", "rate-limit"),
    ("resource_exhausted", "rate-limit"),
    ("out of credit", "rate-limit"),
    ("insufficient quota", "rate-limit"),
    ("over quota", "rate-limit"),
    ("subscribe to go", "rate-limit"),
    ("retrying in", "rate-limit"),
    ("rate limited", "rate-limit"),
    ("rate-limited", "rate-limit"),
    // Fatal provider request failures (opencode surfaces these as
    // `Error from provider (Console): Upstream request failed:
    // [invalid_request_error] ...`). Unlike overloads they settle fast
    // and never auto-retry to success — the final card must show them,
    // never a stale prior reply.
    ("error from provider", ERROR_KIND),
    ("upstream request failed", ERROR_KIND),
    ("invalid_request", ERROR_KIND),
    ("encrypted_content", ERROR_KIND),
    ("was not issued to this caller", ERROR_KIND),
    // Opencode's internal retry UI (`… [retrying attempt #N]`): specific
    // enough to stand alone, but transient — stuck-gated `provider`, never
    // quota. (Bare `retrying` stays WEAK + context-gated below so prose
    // like "retrying the layout pass" stays silent.)
    ("retrying attempt", "provider"),
    // Google-style auth denial (agy): specific enough to stand alone,
    // and "denied" is a CONTEXT marker so it must not stay WEAK (it
    // would be self-contextual while "unauthorized" is not). Both
    // spacings: gRPC uses PERMISSION_DENIED.
    ("permission denied", "auth"),
    ("permission_denied", "auth"),
];

/// Fatal provider-failure markers (lowercase substrings). Shared by the
/// chrome filter bypass and settle arbitration so a framed or
/// fast-settling provider error is never dropped or shadowed.
pub(crate) const FATAL_PROVIDER_MARKERS: &[&str] = &[
    "error from provider",
    "upstream request failed",
    "invalid_request",
    "encrypted_content",
    "was not issued to this caller",
];

/// Weak patterns: common words that also occur in normal prose — they
/// only count when the screen carries error/retry context (see
/// [`CONTEXT`]). Each carries its episode kind.
///
/// NOTE: bare retry words (`retrying`, `retry in`, `will retry`) map to
/// `provider`, NOT `rate-limit`. Opencode prints `[retrying attempt #N]`
/// for every transient upstream timeout (which usually succeeds a few
/// seconds later) — paging that as "usage limit hit" spams one card per
/// flap and gives the wrong fix-it hint. True quota stalls carry
/// quota-specific strings (STRONG above, or `rate limit`/`429`/billing
/// below) and still buzz immediately as `rate-limit`.
pub(crate) const WEAK: &[(&str, &str)] = &[
    ("rate limit", "rate-limit"),
    ("ratelimit", "rate-limit"),
    (" 429", "rate-limit"),
    ("(429", "rate-limit"),
    ("429 ", "rate-limit"),
    ("billing", "rate-limit"),
    ("payment required", "rate-limit"),
    ("unauthorized", "auth"),
    ("invalid api key", "auth"),
    // NOTE: bare "authentication" is deliberately NOT here. Agents
    // routinely DISCUSS authentication in working prose ("handles
    // authentication requirements…") while unrelated tool output carries
    // error context — paging that as "agent auth failed" spams one card
    // per episode with a wrong fix-it hint. Real auth failures carry a
    // specific marker above (permission_denied, invalid api key,
    // login/session/token …), which all stay.
    ("token expired", "auth"),
    ("session expired", "auth"),
    ("login required", "auth"),
    ("overloaded", "provider"),
    ("capacity", "provider"),
    ("try again later", "provider"),
    ("service unavailable", "provider"),
    ("upstream", "provider"),
    ("provider error", "provider"),
    ("model unavailable", "provider"),
    // Generic retry/timeout chatter: transient, stuck-gated, never quota.
    ("retrying", "provider"),
    ("retry in", "provider"),
    ("will retry", "provider"),
    ("timed out", "provider"),
    ("timed-out", "provider"),
    ("timeout", "provider"),
];

/// Error/retry context markers: a weak hit only counts when some line
/// on the screen carries one of these (so prose like "rate limits
/// ensure fair use" stays silent). NOTE: "limit"/"quota" are deliberately
/// absent — they would make weak patterns self-satisfying. "retry" is
/// absent for the same reason: it is a substring of the `retrying`
/// weak patterns, so it would make every retry line self-contextual and
/// prose like "retrying the layout pass" would page. Genuine retry UI
/// (`[retrying attempt #N]`) is STRONG and needs no context.
pub(crate) const CONTEXT: &[&str] = &[
    "error",
    "fail",
    "exceed",
    "denied",
    "unable",
    "cannot",
    "could not",
    "sorry",
    "⚠",
    "⛔",
    "❗",
    "🚫",
];
