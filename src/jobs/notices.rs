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
pub struct LimitHit {
    /// Stable per-episode signature: `rate-limit` | `auth` | `provider`.
    /// Dedup keys on this (NOT the excerpt — countdowns like
    /// `retrying in 12s` change every second and must not re-alert).
    pub kind: &'static str,
    /// The matched screen line (trimmed, capped) for the card body.
    pub excerpt: String,
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
];

/// Weak patterns: common words that also occur in normal prose — they
/// only count when the screen carries error/retry context (see
/// [`CONTEXT`]). Each carries its episode kind.
const WEAK: &[(&str, &str)] = &[
    ("rate limit", "rate-limit"),
    ("ratelimit", "rate-limit"),
    ("retry in", "rate-limit"),
    ("will retry", "rate-limit"),
    ("retrying", "rate-limit"),
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
];

/// Error/retry context markers: a weak hit only counts when some line
/// on the screen carries one of these (so prose like "rate limits
/// ensure fair use" stays silent). NOTE: "limit"/"quota" are deliberately
/// absent — they would make weak patterns self-satisfying.
const CONTEXT: &[&str] = &[
    "error", "fail", "exceed", "retry", "denied", "unable", "cannot",
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

fn clip(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= 180 {
        return t.to_string();
    }
    format!("{}…", t.chars().take(179).collect::<String>())
}

/// Buzzing card body for a limit episode. Names the pane so DM owners
/// with several agents know which one stalled.
pub fn limit_card_text(pane: &str, hit: &LimitHit) -> String {
    let (head, hint) = match hit.kind {
        "auth" => (
            "⚠️ agent auth failed",
            "Check login / API key on the PC, then prompt again.",
        ),
        "provider" => (
            "⚠️ provider overloaded",
            "The agent retries on its own — wait or try /model later.",
        ),
        _ => (
            "⚠️ usage limit hit — agent is auto-retrying",
            "Wait for reset, or /model to switch to a free Zen model.",
        ),
    };
    let mut text = format!("{head} [{pane}]");
    if !hit.excerpt.is_empty() {
        text.push_str(&format!("\n\n> {}", hit.excerpt));
    }
    text.push_str(&format!("\n\n{hint}\n• /read — full output\n• /cancel — stop the run"));
    text
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
}
