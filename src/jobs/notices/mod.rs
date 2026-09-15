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
pub use card::limit_card_text;

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

/// Strong patterns: specific enough to alert on their own. All
/// lowercase, matched as substrings against the lowercased line.
const STRONG: &[(&str, &str)] = &[
    ("free usage exceeded", "rate-limit"),
    ("usage exceeded", "rate-limit"),
    ("limit exceeded", "rate-limit"),
    ("rate limit exceeded", "rate-limit"),
    ("too many requests", "rate-limit"),
    ("quota exceeded", "rate-limit"),
    ("quota exhausted", "rate-limit"),
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
];

/// Fatal provider-failure markers (lowercase substrings). Shared by the
/// chrome filter bypass and settle arbitration so a framed or
/// fast-settling provider error is never dropped or shadowed.
const FATAL_PROVIDER_MARKERS: &[&str] = &[
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
const WEAK: &[(&str, &str)] = &[
    ("rate limit", "rate-limit"),
    ("ratelimit", "rate-limit"),
    (" 429", "rate-limit"),
    ("(429", "rate-limit"),
    ("429 ", "rate-limit"),
    ("billing", "rate-limit"),
    ("payment required", "rate-limit"),
    ("unauthorized", "auth"),
    ("invalid api key", "auth"),
    ("authentication", "auth"),
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
const CONTEXT: &[&str] = &[
    "error", "fail", "exceed", "denied", "unable", "cannot",
    "could not", "sorry", "⚠", "⛔", "❗", "🚫",
];

/// Scan a raw pane screen for limit/quota/provider stalls. Strong hits
/// win immediately; weak hits need screen-wide error context. Returns
/// the first hit (topmost line) so the card quotes what the user saw.
pub fn detect_limit(lines: &[String]) -> Option<LimitHit> {
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    for (i, l) in lower.iter().enumerate() {
        if let Some(kind) = STRONG.iter().find(|(p, _)| l.contains(p)).map(|(_, k)| *k) {
            return Some(LimitHit {
                kind,
                excerpt: clip(&lines[i]),
            });
        }
    }
    let context = lower.iter().any(|l| CONTEXT.iter().any(|c| l.contains(c)));
    if !context {
        return None;
    }
    for (i, l) in lower.iter().enumerate() {
        if let Some(kind) = WEAK.iter().find(|(p, _)| l.contains(p)).map(|(_, k)| *k) {
            return Some(LimitHit {
                kind,
                excerpt: clip(&lines[i]),
            });
        }
    }
    None
}

/// A single screen line carrying a fatal provider request failure
/// (never TUI chrome — survives framing/filtering and shadows nothing).
pub fn is_provider_failure_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    FATAL_PROVIDER_MARKERS.iter().any(|m| lower.contains(m))
}

/// Any line on the screen is a fatal provider failure.
pub fn screen_has_provider_failure(lines: &[String]) -> bool {
    lines.iter().any(|l| is_provider_failure_line(l))
}

fn clip(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= 180 {
        return t.to_string();
    }
    format!("{}…", t.chars().take(179).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_detects_opencode_free_usage() {
        let screen = v(&[
            "  opencode",
            "  Free usage exceeded, subscribe to Go [retrying in 42s]",
            "  ⠋ working…",
        ]);
        let hit = detect_limit(&screen).expect("must detect");
        assert_eq!(hit.kind, "rate-limit");
        assert!(hit.excerpt.contains("Free usage exceeded"));
    }

    #[test]
    fn test_strong_case_insensitive() {
        let hit = detect_limit(&v(&["QUOTA EXCEEDED for model x"])).expect("must detect");
        assert_eq!(hit.kind, "rate-limit");
    }

    #[test]
    fn test_weak_needs_context() {
        // Prose about rate limits with no error words stays silent.
        assert!(detect_limit(&v(&["rate limits ensure fair use"])).is_none());
        // Same phrase next to an error retries → alert.
        let hit = detect_limit(&v(&["error: rate limit hit", "backing off"])).expect("must detect");
        assert_eq!(hit.kind, "rate-limit");
    }

    #[test]
    fn test_auth_kind() {
        let hit = detect_limit(&v(&["error: invalid api key for provider"])).expect("must detect");
        assert_eq!(hit.kind, "auth");
    }

    #[test]
    fn test_clean_screen_no_hit() {
        assert!(detect_limit(&v(&["hello there", "  Thought · 300ms"])).is_none());
        assert!(detect_limit(&[]).is_none());
    }

    #[test]
    fn test_excerpt_clipped() {
        let long = "x".repeat(300);
        let hit = detect_limit(&v(&[&format!("quota exceeded {long}")])).expect("must detect");
        assert!(hit.excerpt.chars().count() <= 180);
    }

    #[test]
    fn test_card_text_names_pane() {
        let hit = LimitHit {
            kind: "rate-limit",
            excerpt: "Free usage exceeded".into(),
        };
        let text = limit_card_text("wG:p1", &hit);
        assert!(text.contains("wG:p1"));
        assert!(text.contains("Free usage exceeded"));
        assert!(text.contains("/model"));
    }

    #[test]
    fn test_detects_fatal_provider_encrypted_content() {
        let screen = v(&[
            "Error from provider (Console): Upstream request failed: [invalid_request_error] reasoning `encrypted_content` was not issued to this caller",
        ]);
        let hit = detect_limit(&screen).expect("fatal provider error must detect");
        assert_eq!(hit.kind, ERROR_KIND);
        assert!(screen_has_provider_failure(&screen));
        assert!(is_provider_failure_line(&screen[0]));
    }

    #[test]
    fn test_fatal_markers_case_insensitive() {
        assert!(is_provider_failure_line("ERROR FROM PROVIDER (console) boom"));
        assert!(is_provider_failure_line("  ┃ Error from provider (Console): fail"));
        assert!(!is_provider_failure_line("hello there"));
    }

    #[test]
    fn test_transient_timeout_retry_is_provider_not_quota() {
        // The spam-loop shape: transient upstream timeout with an internal
        // retry counter. Must NEVER page as usage-limit/quota.
        let screen = v(&[
            "⬝⬝⬝■■■■■ Provider response headers timed out after 300000ms [retrying attempt #1]  esc interrupt",
        ]);
        let hit = detect_limit(&screen).expect("must detect as stall");
        assert_eq!(hit.kind, "provider");
        assert!(is_stuck_gated(hit.kind));
        assert!(!is_stuck_gated("rate-limit"));
    }

    #[test]
    fn test_bare_retrying_needs_context_and_is_provider() {
        // No error context → silent even for retry words.
        assert!(detect_limit(&v(&["retrying the layout pass"])).is_none());
        let hit = detect_limit(&v(&["error: upstream retrying attempt #2"])).expect("must detect");
        assert_eq!(hit.kind, "provider");
    }

    #[test]
    fn test_quota_retrying_in_still_rate_limit() {
        let hit = detect_limit(&v(&["Free usage exceeded, subscribe to Go [retrying in 42s]"]))
            .expect("quota must detect");
        assert_eq!(hit.kind, "rate-limit");
    }
}
