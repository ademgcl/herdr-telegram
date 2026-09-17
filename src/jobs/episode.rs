//! Mid-run buzz episodes: which stall banner already alerted this
//! watcher. `error`, `provider`, `rate-limit` and WEAK `auth` matches
//! buzz only when stuck — transient flashes (upstream blips, 429
//! auto-retry bursts, common words + ambient context in agent prose or
//! log dumps) recover into the final reply, and settle arbitration
//! already surfaces terminal errors there. STRONG-`auth` buzzes
//! immediately: specific denials persist until login is fixed. The
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
use super::notices::types::ERROR_KIND;
use std::time::{Duration, Instant};

/// Ticks with a gated (`error`/`provider`/`rate-limit`/WEAK-`auth`)
/// banner and no settle before buzzing once.
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

    /// A fatal provider error has been stuck since `now` minus the full
    /// gate: the run is effectively over even when herdr keeps sampling
    /// settled kinds too briefly to arm the report timer (working↔blocked
    /// ↔idle flap around a fatal error would else loop the watcher
    /// forever on a frozen "working" card). Settle uses this to commit
    /// at once, like `blocked`. Delivery-independent on purpose: a dead
    /// Telegram must not also block terminal delivery (finalize retries
    /// sends itself). `now` is a parameter (not captured) so tests can
    /// time-travel without sleeping the gate out.
    pub fn error_stuck(&self, now: std::time::Instant) -> bool {
        self.kind.as_deref() == Some(ERROR_KIND)
            && self
                .since
                .is_some_and(|t| now.duration_since(t) >= std::time::Duration::from_secs(STUCK_SECS))
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
#[path = "episode_tests.rs"]
mod tests;
