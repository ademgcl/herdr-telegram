//! Screen scanning for limit/quota/provider stalls. Pure std-only.
use super::patterns::{CONTEXT, FATAL_PROVIDER_MARKERS, STRONG, WEAK, best_hit, normalize_line};
use super::types::LimitHit;

/// Scan a raw pane screen for limit/quota/provider stalls. Strong hits
/// stand alone; weak hits need screen-wide error context. Multiple
/// banners can co-exist as the TUI scrolls (a stale transient retry line
/// above a fresh quota banner): ranking is by (priority, freshness) —
/// top-priority kinds (`rate-limit`/STRONG-`auth`) outrank stuck-gated ones
/// across BOTH tables, ties break bottommost (freshest) — so the card
/// quotes what the user saw last, never a scrolled-off transient.
pub fn detect_limit(lines: &[String]) -> Option<LimitHit> {
    // Normalized before matching so quota banners with single-e
    // misspellings (`exceded`) still hit; `rfree`-style prefixes already
    // match via substring (`free usage exceeded`).
    let lower: Vec<String> = lines
        .iter()
        .map(|l| normalize_line(&l.to_lowercase()))
        .collect();
    let strong_hit = best_hit(&lower, STRONG, true);
    let context = lower.iter().any(|l| CONTEXT.iter().any(|c| l.contains(c)));
    let weak_hit = if context {
        best_hit(&lower, WEAK, false)
    } else {
        None
    };
    // Cross-table pick: lowest priority wins, ties break bottommost.
    // (A stale STRONG transient must not shadow a fresh WEAK quota, and
    // vice versa — priority first, recency second, table never.)
    let mut best: Option<(u8, usize, &'static str, bool)> = None;
    for (hit, strong) in [strong_hit, weak_hit].into_iter().zip([true, false]) {
        if let Some((i, kind)) = hit {
            let p = super::patterns::kind_priority(kind, strong);
            let better = match best {
                None => true,
                Some((bp, bi, _, _)) => p < bp || (p == bp && i > bi),
            };
            if better {
                best = Some((p, i, kind, strong));
            }
        }
    }
    best.map(|(_, i, kind, strong)| LimitHit {
        kind,
        excerpt: clip(&lines[i]),
        strong,
    })
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

/// Trim + 180-char cap for stall excerpts. Shared with the settled-pane
/// tail scan (`limit_decide::detect_tail_with_context`) so both rank and
/// quote identically.
pub(crate) fn clip(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= 180 {
        return t.to_string();
    }
    format!("{}…", t.chars().take(179).collect::<String>())
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;

