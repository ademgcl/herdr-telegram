//! Screen scanning for limit/quota/provider stalls. Pure std-only.
use super::patterns::{CONTEXT, FATAL_PROVIDER_MARKERS, STRONG, WEAK};
use super::types::LimitHit;

/// Scan a raw pane screen for limit/quota/provider stalls. Strong hits
/// win immediately; weak hits need screen-wide error context. Multiple
/// banners can co-exist as the TUI scrolls (a stale transient retry line
/// above a fresh quota banner): immediate kinds (`rate-limit`/`auth`)
/// outrank stuck-gated ones, ties break bottommost (freshest) — so the
/// card quotes what the user saw last, never a scrolled-off transient.
pub fn detect_limit(lines: &[String]) -> Option<LimitHit> {
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    if let Some((i, kind)) = best_hit(&lower, STRONG) {
        return Some(LimitHit {
            kind,
            excerpt: clip(&lines[i]),
        });
    }
    let context = lower.iter().any(|l| CONTEXT.iter().any(|c| l.contains(c)));
    if !context {
        return None;
    }
    if let Some((i, kind)) = best_hit(&lower, WEAK) {
        return Some(LimitHit {
            kind,
            excerpt: clip(&lines[i]),
        });
    }
    None
}

/// Best (line index, kind) in a table: lowest kind priority wins, ties
/// break to the bottommost (freshest) line.
fn best_hit(
    lower: &[String],
    table: &'static [(&'static str, &'static str)],
) -> Option<(usize, &'static str)> {
    let mut best: Option<(u8, usize, &'static str)> = None;
    for (i, l) in lower.iter().enumerate() {
        if let Some(kind) = table.iter().find(|(p, _)| l.contains(p)).map(|(_, k)| *k) {
            let better = match best {
                None => true,
                // Lower priority wins; ties break bottommost (freshest).
                Some((p, j, _)) => kind_priority(kind) < p || (kind_priority(kind) == p && i > j),
            };
            if better {
                best = Some((kind_priority(kind), i, kind));
            }
        }
    }
    best.map(|(_, i, kind)| (i, kind))
}

/// Immediate quota/auth stalls page at once; transient-prone banners
/// (`provider`/`error`) are stuck-gated by the caller instead.
fn kind_priority(kind: &str) -> u8 {
    match kind {
        "rate-limit" | "auth" => 0,
        _ => 1,
    }
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
    use super::super::types::{ERROR_KIND, is_stuck_gated};
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
        assert!(is_provider_failure_line(
            "ERROR FROM PROVIDER (console) boom"
        ));
        assert!(is_provider_failure_line(
            "  ┃ Error from provider (Console): fail"
        ));
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
        let hit = detect_limit(&v(&[
            "Free usage exceeded, subscribe to Go [retrying in 42s]",
        ]))
        .expect("quota must detect");
        assert_eq!(hit.kind, "rate-limit");
    }

    #[test]
    fn test_fresh_quota_beats_stale_transient() {
        // Scrollback keeps an old transient retry line above the fresh
        // quota banner: the immediate kind must win, not topmost order.
        let hit = detect_limit(&v(&[
            "⬝⬝⬝ Provider response headers timed out after 300000ms [retrying attempt #1]",
            "Free usage exceeded, subscribe to Go [retrying in 42s]",
        ]))
        .expect("must detect");
        assert_eq!(hit.kind, "rate-limit");
        assert!(hit.excerpt.contains("Free usage exceeded"));
    }

    #[test]
    fn test_same_priority_breaks_bottommost() {
        // Two gated banners: the freshest (bottommost) represents.
        let hit = detect_limit(&v(&[
            "upstream error on attempt 1",
            "upstream error on attempt 9",
        ]))
        .expect("must detect");
        assert_eq!(hit.kind, "provider");
        assert!(hit.excerpt.contains("attempt 9"));
    }
}
