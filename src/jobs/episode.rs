//! Mid-run buzz episodes: which stall banner already alerted this
//! watcher. Provider fatals (`error`) buzz only when stuck — transient
//! flashes recover into the final reply, and settle arbitration already
//! surfaces terminal errors there. The stuck gate is Instant-based
//! (watcher wakes are event-driven, not periodic).
use std::time::{Duration, Instant};
use super::notices::{LimitHit, ERROR_KIND};

/// Ticks with an `error` banner and no settle before buzzing once.
const STUCK_SECS: u64 = 90;

pub struct BuzzEpisode {
    kind: Option<String>,
    since: Option<Instant>,
    alerted: bool,
}

impl BuzzEpisode {
    pub fn new() -> Self {
        Self { kind: None, since: None, alerted: false }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Current detection (or None) → the hit to report this tick, if any.
    /// New kinds report at once (except `error`); repeats stay silent;
    /// a missing banner resets the episode.
    pub fn tick<'a>(&mut self, hit: Option<&'a LimitHit>, now: Instant) -> Option<&'a LimitHit> {
        let Some(hit) = hit else {
            self.reset();
            return None;
        };
        if self.kind.as_deref() == Some(hit.kind) {
            // Same episode: only a stuck, unalerted fatal re-fires.
            if hit.kind == ERROR_KIND
                && !self.alerted
                && self.since.is_some_and(|t| now.duration_since(t) >= Duration::from_secs(STUCK_SECS))
            {
                self.alerted = true;
                return Some(hit);
            }
            return None;
        }
        // New episode (banner returned or kind changed).
        self.kind = Some(hit.kind.to_string());
        self.since = (hit.kind == ERROR_KIND).then(|| now);
        self.alerted = false;
        if hit.kind == ERROR_KIND {
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
        let p = hit("provider");
        assert!(ep.tick(Some(&p), t).is_some());
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
    fn test_banner_leave_and_kind_change_reset() {
        let mut ep = BuzzEpisode::new();
        let t0 = Instant::now();
        let e = hit(ERROR_KIND);
        assert!(ep.tick(Some(&e), t0).is_none());
        assert!(ep.tick(None, t0 + Duration::from_secs(200)).is_none());
        // Fresh episode after the banner left: silent again, timer restarted.
        assert!(ep.tick(Some(&e), t0 + Duration::from_secs(200)).is_none());
        assert!(ep.tick(Some(&e), t0 + Duration::from_secs(291)).is_some());
        // Kind change resets the timer too.
        let r = hit("rate-limit");
        assert!(ep.tick(Some(&r), t0 + Duration::from_secs(300)).is_some());
        assert!(ep.tick(Some(&e), t0 + Duration::from_secs(300)).is_none());
    }
}
