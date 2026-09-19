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
    assert!(is_stuck_gated("rate-limit"));
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
fn test_google_resource_exhausted_is_rate_limit() {
    // agy/Google gRPC quota shape (underscores, no "quota exceeded"
    // wording): must buzz as rate-limit, not provider. Bare form
    // isolates the new pattern — it must fire on its own.
    let hit = detect_limit(&v(&[
        "GenerateContent failed: RESOURCE_EXHAUSTED for gemini-3-flash",
    ]))
    .expect("google quota must detect");
    assert_eq!(hit.kind, "rate-limit");
}

#[test]
fn test_google_permission_denied_is_auth() {
    // Bare gRPC shape (underscores only, no spaced wording).
    let hit = detect_limit(&v(&[
        "GenerateContent failed: PERMISSION_DENIED for gemini-3-flash",
    ]))
    .expect("google auth denial must detect");
    assert_eq!(hit.kind, "auth");
}

#[test]
fn test_codex_usage_limit_is_rate_limit() {
    // Codex limit banner (■ You've hit your usage limit. ...): must buzz as rate-limit,
    // and is marked as fatal provider failure so settled screens keep it over footers.
    let screen = v(&[
        "■ You've hit your usage limit. You can continue using Codex after your limit resets at 8:00 PM.",
        "gpt-5.6-sol medium · ~/develop/ws/omnitrack · Resume product brief",
    ]);
    let hit = detect_limit(&screen).expect("codex limit banner must detect");
    assert_eq!(hit.kind, "rate-limit");
    assert!(hit.strong);
    assert!(super::super::types::needs_stuck_gate(&hit));
    assert!(screen_has_provider_failure(&screen));
    assert!(is_provider_failure_line(&screen[0]));
}

#[test]
fn test_fresh_quota_beats_stale_transient() {
    // Scrollback keeps an old transient retry line above the fresh
    // quota banner: the top-priority kind must win, not topmost order.
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

#[test]
fn test_stale_weak_auth_never_outranks_fresh_provider() {
    // WEAK `auth` ranks gated now: a stale auth-prose line above a
    // fresh provider banner loses to the freshest line.
    let hit = detect_limit(&v(&[
        "warning: session expired, see error log",
        "upstream error on attempt 9",
    ]))
    .expect("must detect");
    assert_eq!(hit.kind, "provider");
    assert!(hit.excerpt.contains("attempt 9"));
}

#[test]
fn test_agent_auth_prose_with_ambient_error_stays_silent() {
    // Live agy false positive: the agent DISCUSSING authentication
    // in working prose while unrelated tool output carries error
    // context must never page as "agent auth failed".
    let screen = v(&[
        "> I'm currently investigating the /orbit/clusters endpoint, specifically how the run-orbit.sh script handles authentication requirements like orbi...",
        "⡿ Running command...",
        "  Build failed: 2 tests red",
    ]);
    assert!(detect_limit(&screen).is_none());
}

#[test]
fn test_auth_provenance_strong_vs_weak() {
    // STRONG wording stands alone and pages at once…
    let hit = detect_limit(&v(&[
        "GenerateContent failed: PERMISSION_DENIED for gemini-3-flash",
    ]))
    .expect("must detect");
    assert_eq!(hit.kind, "auth");
    assert!(hit.strong);
    // …while WEAK wording needs context and is stuck-gated.
    let hit = detect_limit(&v(&["error: session expired, retrying login"])).expect("must detect");
    assert_eq!(hit.kind, "auth");
    assert!(!hit.strong);
    assert!(super::super::types::needs_stuck_gate(&hit));
}

#[test]
fn test_weak_rate_limit_is_stuck_gated() {
    // Gating is uniform now: a WEAK quota signal stays silent until it
    // persists (transient 429/auto-retry flashes recover) — and priority
    // still beats recency across lines (quota first, fresher provider
    // second).
    let hit = detect_limit(&v(&[
        "error: upstream replied (429) trouble",
        "upstream error on attempt 9",
    ]))
    .expect("must detect");
    assert_eq!(hit.kind, "rate-limit");
    assert!(!hit.strong);
    assert!(super::super::types::needs_stuck_gate(&hit));
    assert!(hit.excerpt.contains("429"));
}

#[test]
fn test_stale_strong_transient_loses_to_fresh_weak_quota() {
    // Cross-table pick: a stale STRONG transient must not shadow a
    // fresh WEAK quota — priority first, recency second, table never.
    let hit = detect_limit(&v(&[
        "⬝⬝⬝ Provider response headers timed out after 300000ms [retrying attempt #1]  esc interrupt",
        "error: upstream replied (429) trouble",
    ]))
    .expect("must detect");
    assert_eq!(hit.kind, "rate-limit");
    assert!(!hit.strong);
    assert!(hit.excerpt.contains("429"));
}
