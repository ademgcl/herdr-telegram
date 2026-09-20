//! Pure router guards: stale/setup notice windows + log sanitizer
//! (split from `router`, 300-line file limit). Single source for the
//! burst-guard windows used by the update pump.
use crate::types::{NAGGED_SECS, STALE_SECS};
use std::time::Instant;

/// Stale-notice burst guard: a boot burst queues N stale messages and
/// each must not send its own "please resend" (serial and slow — fresh
/// updates stall behind the spam). One notice per (chat, thread) per
/// STALE_SECS; a later genuine stall still notifies. Pure for tests.
pub(crate) fn stale_notice_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|t| now.duration_since(t).as_secs() >= STALE_SECS)
        .unwrap_or(true)
}

/// Setup-note guard: same shape, daily window — an unconfigured group
/// reminds once a day, never spams, never mutes forever. Pure for tests.
pub(crate) fn setup_note_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|t| now.duration_since(t).as_secs() >= NAGGED_SECS)
        .unwrap_or(true)
}

/// Strip control + bidi/format chars from user-controlled titles before
/// logging: logged titles must never forge log lines or inject terminal
/// escapes.
pub(crate) fn log_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control()
                || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' | '\u{FEFF}')
            {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_safe_strips_newlines() {
        assert_eq!(log_safe("plain"), "plain");
        assert_eq!(log_safe("a\nb\rc"), "a b c");
        assert_eq!(log_safe("[x]\nFAKE LOG"), "[x] FAKE LOG");
        assert_eq!(log_safe("a\x1bb[2Jc\x07d\u{202e}e"), "a b[2Jc d e");
    }

    #[test]
    fn test_stale_notice_due_first_then_quiet_then_due() {
        let now = Instant::now();
        assert!(stale_notice_due(None, now));
        assert!(!stale_notice_due(Some(now), now));
        let recent = now - std::time::Duration::from_secs(10);
        assert!(!stale_notice_due(Some(recent), now));
        let old = now - std::time::Duration::from_secs(STALE_SECS + 1);
        assert!(stale_notice_due(Some(old), now));
    }

    #[test]
    fn test_setup_note_due_daily_window() {
        let now = Instant::now();
        assert!(setup_note_due(None, now));
        assert!(!setup_note_due(Some(now), now));
        let hour = now - std::time::Duration::from_secs(3600);
        assert!(!setup_note_due(Some(hour), now));
        let old = now - std::time::Duration::from_secs(NAGGED_SECS + 1);
        assert!(setup_note_due(Some(old), now));
    }
}
