//! Match tables + ranking for limit/stall detection. Pure, std-only.
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
    ("hit your usage limit", "rate-limit"), // also covers "you've hit…"
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
    "hit your usage limit", // also covers "you've hit…"
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

/// Quota-plausible markers: the typo fix applies only on lines carrying
/// one of these, so bare typo prose elsewhere stays byte-identical and
/// can never newly satisfy WEAK-context matching.
const TYPO_CONTEXT: &[&str] = &[
    "usage", "quota", "limit", "ratelimit", "credit", "billing", "429", "subscribe",
];

/// Normalize a lowercased screen line before matching: collapse the
/// common single-e misspelling (`exceded`/`excede` for `exceeded`/`exceed`)
/// on quota-plausible lines only, so drifted quota banners still match
/// the tables above. Longer misspelling first (`exceded` contains `excede`).
pub(crate) fn normalize_line(l: &str) -> String {
    if !TYPO_CONTEXT.iter().any(|c| l.contains(c)) {
        return l.to_string();
    }
    l.replace("exceded", "exceeded").replace("excede", "exceed")
}
/// Best (line index, kind) in a table: lowest kind priority wins, ties
/// break to the bottommost (freshest) line. Provenance-aware: WEAK `auth`
/// is stuck-gated (see `needs_stuck_gate`), so it ranks with the gated
/// kinds — a stale auth-prose line must not outrank a fresh provider
/// banner just because they share the `auth` name.
pub(crate) fn best_hit(
    lower: &[String],
    table: &'static [(&'static str, &'static str)],
    strong: bool,
) -> Option<(usize, &'static str)> {
    let mut best: Option<(u8, usize, &'static str)> = None;
    for (i, l) in lower.iter().enumerate() {
        // Every pattern on the line competes: first-table-order must not
        // let a gated match shadow an immediate one on the same line.
        let line_best = table
            .iter()
            .filter(|(p, _)| l.contains(*p))
            .map(|(_, k)| *k)
            .min_by_key(|k| kind_priority(k, strong));
        if let Some(kind) = line_best {
            let better = match best {
                None => true,
                // Lower priority wins; ties break bottommost (freshest).
                Some((p, j, _)) => {
                    kind_priority(kind, strong) < p || (kind_priority(kind, strong) == p && i > j)
                }
            };
            if better {
                best = Some((kind_priority(kind, strong), i, kind));
            }
        }
    }
    best.map(|(_, i, kind)| (i, kind))
}

/// Immediate quota/STRONG-auth stalls page at once; WEAK `auth` and
/// transient-prone banners (`provider`/`error`) are stuck-gated by the
/// caller instead. Shared with `detect`'s cross-table pick so both
/// tables rank by one rule.
pub(crate) fn kind_priority(kind: &str, strong: bool) -> u8 {
    match (kind, strong) {
        ("rate-limit", _) | ("auth", true) => 0,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_fixes_exceded_typo() {
        assert_eq!(normalize_line("free usage exceded"), "free usage exceeded");
        assert_eq!(normalize_line("quota excede limit"), "quota exceed limit");
        assert_eq!(normalize_line("free usage exceeded"), "free usage exceeded");
        // Bare typo prose without quota markers stays byte-identical.
        assert_eq!(normalize_line("we exceded expectations"), "we exceded expectations");
    }

    #[test]
    fn test_typo_banner_still_matches_strong() {
        // A drifted/mistyped quota banner must still hit after normalize.
        let raw = ["rfree usage exceded, subscribe to go".to_string()];
        let lower: Vec<String> = raw.iter().map(|l| normalize_line(&l.to_lowercase())).collect();
        let hit = best_hit(&lower, STRONG, true).expect("typo quota must match");
        assert_eq!(hit.1, "rate-limit");
    }
}
