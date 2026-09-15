//! Mid-run buzz episodes: which stall banner already alerted this
//! watcher. `provider` fatals and transient upstream stalls (`error`,
//! `provider`) buzz only when stuck — transient flashes recover into the
//! final reply, and settle arbitration already surfaces terminal errors
//! there. `rate-limit`/`auth` buzz immediately: quota stalls never
//! self-heal. The stuck gate is Instant-based (watcher wakes are
//! event-driven, not periodic).
//!
//! Once-per-episode survives noise: a single clean tick (banner scrolled
//! out of the 40-line tail, one failed RPC, one redraw) does NOT clear
//! the episode — absence must persist for [`CLEAR_MISSES`] consecutive
//! clean reads. Read outages (empty screen) preserve everything and are
//! skipped by the caller via [`BuzzEpisode::note_empty`]. A kind flip
//! must hold for [`KIND_SWITCH_STABLE`] consecutive ticks before it
//! counts as a new episode, so scroll-order oscillation between
//! co-present banners never spams.
use std::time::{Duration, Instant};
use super::notices::{is_stuck_gated, LimitHit, ERROR_KIND};

/// Ticks with a gated (`error`/`provider`) banner and no settle before
/// buzzing once.
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
        Self { kind: None, since: None, alerted: false, misses: 0, pending_kind: None, pending_n: 0 }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Read outage / empty screen: unknown, not clean. Preserves the
    /// episode (kind, stuck timer, alert state) so the next good read
    /// does NOT re-alert. Callers must skip `tick` on empty screens and
    /// call this (a no-op) instead.
    pub fn note_empty(&mut self) {}

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
            // Same episode: only a stuck, unalerted gated banner re-fires.
            if is_stuck_gated(hit.kind)
                && !self.alerted
                && self.since.is_some_and(|t| now.duration_since(t) >= Duration::from_secs(STUCK_SECS))
            {
                self.alerted = true;
                return Some(hit);
            }
            // Immediate kinds fired at episode start; repeats silent.
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
        self.since = is_stuck_gated(hit.kind).then(|| now);
        self.alerted = false;
        self.pending_kind = None;
        self.pending_n = 0;
        if hit.kind == ERROR_KIND || hit.kind == "provider" {
            return None;
        }
        Some(hit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: &'static str) -> LimitHit {
        LimitHit { kind, excerpt: "x".into() }
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
}
