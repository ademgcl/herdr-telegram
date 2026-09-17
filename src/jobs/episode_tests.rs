//! Tests for [`super::BuzzEpisode`] (split: 300-line file limit).
//! `rate-limit` is stuck-gated like `error`/`provider`: transient
//! quota/auto-retry flashes stay silent, only a banner that persists
//! the full gate pages. STRONG `auth` still fires immediately.
use super::super::notices::types::ERROR_KIND;
use super::*;

fn hit(kind: &'static str) -> LimitHit {
    LimitHit {
        kind,
        excerpt: "x".into(),
        strong: true,
    }
}

fn weak_hit(kind: &'static str) -> LimitHit {
    LimitHit {
        kind,
        excerpt: "x".into(),
        strong: false,
    }
}

#[test]
fn test_immediate_kinds_buzz_on_change_only() {
    // STRONG auth pages at once; repeats stay silent.
    let mut ep = BuzzEpisode::new();
    let t = Instant::now();
    let a = hit("auth");
    assert!(ep.tick(Some(&a), t).is_some());
    assert!(ep.tick(Some(&a), t).is_none());
    // A single-tick flip is scroll noise: silent…
    let p = hit("provider");
    assert!(ep.tick(Some(&p), t).is_none());
    assert!(ep.tick(Some(&p), t).is_none());
    // …but held stable it becomes a genuine new episode (gated → silent first).
    assert!(ep.tick(Some(&p), t).is_none());
    // provider is stuck-gated: buzzes only after STUCK_SECS.
    assert!(ep.tick(Some(&p), t + Duration::from_secs(91)).is_some());
}

#[test]
fn test_error_stuck_gate() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let e = hit(ERROR_KIND);
    assert!(ep.tick(Some(&e), t0).is_none());
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(30)).is_none());
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(90)).is_some());
    // Post-once: still present right after stays silent.
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(200)).is_none());
}

#[test]
fn test_rate_limit_stuck_gate() {
    // Transient 429/auto-retry flashes must stay silent; only a banner
    // that persists the full gate pages (then exactly once).
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let r = hit("rate-limit");
    assert!(ep.tick(Some(&r), t0).is_none());
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(30)).is_none());
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(90)).is_some());
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(200)).is_none());
    // WEAK quota wording gates identically (kind-based, not provenance).
    let mut ep2 = BuzzEpisode::new();
    let w = weak_hit("rate-limit");
    assert!(ep2.tick(Some(&w), t0).is_none());
    assert!(ep2.tick(Some(&w), t0 + Duration::from_secs(90)).is_some());
}

#[test]
fn test_error_stuck_gate_state() {
    // Settle bypass reads this, not delivery state.
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    assert!(!ep.error_stuck(t0));
    let e = hit(ERROR_KIND);
    ep.tick(Some(&e), t0);
    assert!(!ep.error_stuck(t0 + Duration::from_secs(30)));
    assert!(ep.error_stuck(t0 + Duration::from_secs(90)));
    // Confirmed-clear resets it.
    ep.tick(None, t0);
    ep.tick(None, t0);
    ep.tick(None, t0);
    assert!(!ep.error_stuck(t0 + Duration::from_secs(200)));
    // Stuck non-error episodes never count (rate-limit recovers).
    let mut ep2 = BuzzEpisode::new();
    let r = hit("rate-limit");
    ep2.tick(Some(&r), t0);
    assert!(!ep2.error_stuck(t0 + Duration::from_secs(500)));
    let mut ep3 = BuzzEpisode::new();
    let a = hit("auth");
    ep3.tick(Some(&a), t0);
    assert!(!ep3.error_stuck(t0 + Duration::from_secs(500)));
}

#[test]
fn test_provider_stuck_gate() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let p = hit("provider");
    // Transient timeout blip: silent until stuck.
    assert!(ep.tick(Some(&p), t0).is_none());
    assert!(ep.tick(Some(&p), t0 + Duration::from_secs(30)).is_none());
    assert!(ep.tick(Some(&p), t0 + Duration::from_secs(90)).is_some());
    assert!(ep.tick(Some(&p), t0 + Duration::from_secs(200)).is_none());
}

#[test]
fn test_single_absence_does_not_reset() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let a = hit("auth");
    assert!(ep.tick(Some(&a), t0).is_some());
    // One clean tick (scroll flap / missed tail): episode survives.
    assert!(ep.tick(None, t0 + Duration::from_secs(5)).is_none());
    assert!(ep.tick(None, t0 + Duration::from_secs(10)).is_none());
    // Banner back without a confirmed clear: still the same episode.
    assert!(ep.tick(Some(&a), t0 + Duration::from_secs(15)).is_none());
}

#[test]
fn test_empty_preserves_episode() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let a = hit("auth");
    assert!(ep.tick(Some(&a), t0).is_some());
    ep.note_empty();
    ep.note_empty();
    assert!(ep.tick(Some(&a), t0 + Duration::from_secs(15)).is_none());
}

#[test]
fn test_sustained_absence_clears() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let a = hit("auth");
    assert!(ep.tick(Some(&a), t0).is_some());
    assert!(ep.tick(None, t0).is_none());
    assert!(ep.tick(None, t0).is_none());
    assert!(ep.tick(None, t0).is_none());
    // Fresh episode after a confirmed clear alerts again.
    assert!(ep.tick(Some(&a), t0).is_some());
}

#[test]
fn test_banner_leave_and_kind_change_reset() {
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let e = hit(ERROR_KIND);
    assert!(ep.tick(Some(&e), t0).is_none());
    assert!(ep.tick(None, t0 + Duration::from_secs(200)).is_none());
    assert!(ep.tick(None, t0 + Duration::from_secs(200)).is_none());
    assert!(ep.tick(None, t0 + Duration::from_secs(200)).is_none());
    // Fresh episode after the banner left: silent again, timer restarted.
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(200)).is_none());
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(291)).is_some());
    // Stable kind change re-arms (3 ticks), then the stuck gate runs.
    let r = hit("rate-limit");
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(300)).is_none());
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(301)).is_none());
    // Third stable tick opens the episode (timer starts, still silent)…
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(302)).is_none());
    // …and 90s of persistence pages.
    assert!(ep.tick(Some(&r), t0 + Duration::from_secs(392)).is_some());
    // Back to error needs stability again first.
    assert!(ep.tick(Some(&e), t0 + Duration::from_secs(393)).is_none());
}

#[test]
fn test_weak_auth_is_stuck_gated() {
    // WEAK wording (prose/log-shaped) must persist before paging;
    // STRONG wording pages at once.
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let w = weak_hit("auth");
    assert!(ep.tick(Some(&w), t0).is_none());
    assert!(ep.tick(Some(&w), t0 + Duration::from_secs(30)).is_none());
    assert!(ep.tick(Some(&w), t0 + Duration::from_secs(90)).is_some());
    // Post-once: still present right after stays silent.
    assert!(ep.tick(Some(&w), t0 + Duration::from_secs(200)).is_none());
}

#[test]
fn test_weak_auth_episode_upgrades_on_strong_wording() {
    // Same-kind STRONG wording inside an unfired WEAK episode proves
    // the denial genuine and fires immediately.
    let mut ep = BuzzEpisode::new();
    let t0 = Instant::now();
    let w = weak_hit("auth");
    let s = hit("auth");
    assert!(ep.tick(Some(&w), t0).is_none());
    assert!(ep.tick(Some(&s), t0 + Duration::from_secs(10)).is_some());
    assert!(ep.tick(Some(&s), t0 + Duration::from_secs(20)).is_none());
}

#[test]
fn test_strong_auth_fires_immediately() {
    let mut ep = BuzzEpisode::new();
    let t = Instant::now();
    let s = hit("auth");
    assert!(ep.tick(Some(&s), t).is_some());
    assert!(ep.tick(Some(&s), t).is_none());
}
