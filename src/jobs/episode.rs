//! Mid-run buzz episodes: which stall banner already alerted this
//! watcher. `provider` fatals, transient upstream stalls (`error`,
//! `provider`) and WEAK `auth` matches (common words + ambient context —
//! agent prose or log dumps trip these transiently) buzz only when
//! stuck — transient flashes recover into the final reply, and settle
//! arbitration already surfaces terminal errors there.
//! `rate-limit`/STRONG-`auth` buzz immediately: quota stalls never
//! self-heal and specific denials persist until login is fixed. The
//! stuck gate is Instant-based (watcher wakes are event-driven, not
//! periodic).
//!
//! Once-per-episode survives noise: a single clean tick (banner scrolled
//! out of the 40-line tail, one failed RPC, one redraw) does NOT clear
//! the episode — absence must persist for [`CLEAR_MISSES`] consecutive
//! clean reads. Read outages (empty screen) preserve everything and are
//! skipped by the caller via [`BuzzEpisode::note_empty`]. A kind flip
//! must hold for [`KIND_SWITCH_STABLE`] consecutive ticks before it
//! counts as a new episode, so scroll-order oscillation between
//! co-present banners never spams.
use super::notices::{LimitHit, needs_stuck_gate};
use std::time::{Duration, Instant};

/// Ticks with a gated (`error`/`provider`/WEAK-`auth`) banner and no
/// settle before buzzing once.
const STUCK_SECS: u64 = 90;
/// Consecutive clean (non-empty, banner-free) reads before the episode
/// clears so the next banner re-alerts.
const CLEAR_MISSES: u32 = 3;
/// Consecutive ticks holding a different kind before it counts as a new
/// episode (scroll-order flips stay silent).
const KIND_SWITCH_STABLE: u32 = 3;

pub struct BuzzEpisode {
    kind: Option<String>,
    since: Option<Instant>,
    alerted: bool,
    misses: u32,
    pending_kind: Option<String>,
    pending_n: u32,
}

impl BuzzEpisode {
    pub fn new() -> Self {
        Self {
            kind: None,
            since: None,
            alerted: false,
            misses: 0,
            pending_kind: None,
            pending_n: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Read outage / empty screen: unknown, not clean. Preserves the
    /// episode (kind, stuck timer, alert state) so the next good read
    /// does NOT re-alert. Callers must skip `tick` on empty screens and
    /// call this (a no-op) instead.
    pub fn note_empty(&mut self) {}

    /// Delivery failed (Telegram send dropped): unmark the alert but keep
    /// kind + stuck timer, so the next tick retries the card immediately
    /// instead of arming a 30-min suppress with nothing delivered.
    pub fn unfire(&mut self) {
        self.alerted = false;
    }

    /// No open episode (fresh watcher or confirmed-clear): the next banner
    /// starts a new episode and pages per its kind rules.
    pub fn is_fresh(&self) -> bool {
        self.kind.is_none()
    }

    /// Current detection (or None for a clean non-empty read) → the hit
    /// to report this tick, if any. New (stable) kinds report at once
    /// except gated ones; repeats stay silent; a missing banner must
    /// persist [`CLEAR_MISSES`] ticks to reset the episode.
    pub fn tick<'a>(&mut self, hit: Option<&'a LimitHit>, now: Instant) -> Option<&'a LimitHit> {
        let Some(hit) = hit else {
            // Clean absent: hysteresis, never instant-reset (single-tick
            // flap from tail-scroll/redraw must not re-arm the alert).
            // A pending kind flip is interrupted by absence.
            self.pending_kind = None;
            self.pending_n = 0;
            self.misses += 1;
            if self.misses >= CLEAR_MISSES {
                self.reset();
            }
            return None;
        };
        self.misses = 0;
        if self.kind.as_deref() == Some(hit.kind) {
            // Same episode: pending flip (if any) is over.
            self.pending_kind = None;
            self.pending_n = 0;
            // Fire once: immediately for immediate hits, after the stuck
            // interval for gated ones. A WEAK-`auth` episode that upgrades
            // to STRONG wording (same kind) still fires — the denial just
            // proved itself genuine.
            if !self.alerted
                && (!needs_stuck_gate(hit)
                    || self
                        .since
                        .is_some_and(|t| now.duration_since(t) >= Duration::from_secs(STUCK_SECS)))
            {
                self.alerted = true;
                return Some(hit);
            }
            return None;
        }
        // Different kind while an episode is open: require stability so
        // co-present banners (`retrying` + `upstream` lines swapping
        // topmost place as the TUI scrolls) don't each re-alert.
        if self.kind.is_some() {
            if self.pending_kind.as_deref() == Some(hit.kind) {
                self.pending_n += 1;
            } else {
                self.pending_kind = Some(hit.kind.to_string());
                self.pending_n = 1;
            }
            if self.pending_n < KIND_SWITCH_STABLE {
                return None;
            }
        }
        // New stable episode (banner returned after a confirmed clear,
        // or a kind held long enough to be genuine).
        self.kind = Some(hit.kind.to_string());
        self.since = needs_stuck_gate(hit).then_some(now);
        self.pending_kind = None;
        self.pending_n = 0;
        // Gated kinds start their stuck timer silently; immediate kinds
        // fire at once and mark themselves alerted so repeats stay silent.
        if needs_stuck_gate(hit) {
            self.alerted = false;
            return None;
        }
        self.alerted = true;
        Some(hit)
    }
}

#[cfg(test)]
mod tests {
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
    fn test_plain_kinds_buzz_on_change_only() {
        let mut ep = BuzzEpisode::new();
        let t = Instant::now();
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t).is_some());
        assert!(ep.tick(Some(&r), t).is_none());
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
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t0).is_some());
        // One clean tick (scroll flap / missed tail): episode survives.
        assert!(ep.tick(None, t0 + Duration::from_secs(5)).is_none());
        assert!(ep.tick(None, t0 + Duration::from_secs(10)).is_none());
        // Banner back without a confirmed clear: still the same episode.
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(15)).is_none());
    }

    #[test]
    fn test_empty_preserves_episode() {
        let mut ep = BuzzEpisode::new();
        let t0 = Instant::now();
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t0).is_some());
        ep.note_empty();
        ep.note_empty();
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(15)).is_none());
    }

    #[test]
    fn test_sustained_absence_clears() {
        let mut ep = BuzzEpisode::new();
        let t0 = Instant::now();
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t0).is_some());
        assert!(ep.tick(None, t0).is_none());
        assert!(ep.tick(None, t0).is_none());
        assert!(ep.tick(None, t0).is_none());
        // Fresh episode after a confirmed clear alerts again.
        assert!(ep.tick(Some(&r), t0).is_some());
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
        // Stable kind change re-arms (3 ticks), then immediate kinds fire.
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(300)).is_none());
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(301)).is_none());
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(302)).is_some());
        // Back to error needs stability again first.
        assert!(ep.tick(Some(&e), t0 + Duration::from_secs(303)).is_none());
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
}
