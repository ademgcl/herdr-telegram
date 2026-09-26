//! Final-card arbitration: stream vs settled screen. Split from
//! `finalize` (300-line file limit).
use crate::{jobs::segment::final_block, jobs::stream::join_trimmed};

/// Minimum streamed body trusted outright. Below this the stream is
/// assumed starved (alternate-screen TUIs serve chrome-only tails while
/// working) and the settled screen arbitrates. Also the stale-body
/// floor (see stream::is_stale_body): short replies must never die on
/// a coincidental scrollback match — single source, never re-literal.
pub(crate) const STREAM_MIN_CHARS: usize = 40;

/// Choose the final card body. The stream usually wins outright — its
/// rolling baseline (anchored at prompt time) excludes earlier turns.
/// But a starved stream must not shadow the real answer with a stray
/// line: when it yields only a fragment, the settled screen wins if it
/// is longer AND contains the fragment (reflow-proof: compared
/// whitespace-squashed, since streaming and settle reads wrap lines
/// differently). An unrelated longer screen — e.g. a coalesced
/// follow-up turn — never displaces the stream.
pub fn select_final_body(acc: &[String], screen: &[String], prompt: &str) -> String {
    let acc_body = join_trimmed(&final_block(acc, prompt));
    let turn = final_block(screen, prompt);
    let screen_body = join_trimmed(&turn);
    // Fatal provider errors settle fast (often before the stream sees
    // them) while `acc` still holds the prior turn. A settled error must
    // never lose to a stale stream — otherwise Telegram repeats the old
    // answer and the error vanishes. When both carry failures the fresh
    // settled screen still wins (it is the current turn).
    // Turn-scoped scan (final_block): a PRIOR turn's failure line in
    // the scrollback must never flag the current turn as failed — the
    // whole-screen scan did exactly that and the screen took over a
    // healthy stream.
    let screen_failed = crate::jobs::notices::screen_has_provider_failure(&turn)
        || crate::jobs::notices::is_provider_failure_line(&screen_body);
    if screen_failed && !screen_body.is_empty() {
        return screen_body;
    }
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if acc_body.chars().count() >= STREAM_MIN_CHARS {
        // Healthy stream — but it may be TRUNCATED (last burst landed
        // after the final stream tick, so the settled screen holds the
        // same answer PLUS its tail). When the screen starts with the
        // whole stream reflow-proof and is substantially longer, the
        // extra is the missing tail, not scrollback: take the screen.
        // Prefix (never contains): a contains-match also fires when
        // stale scrollback heads the screen (old-turn + fresh), and
        // returning the screen then leaks the prior turn as the reply.
        // Unrelated longer screens (no prefix) never displace.
        let acc_sq = squash(&acc_body);
        let screen_sq = squash(&screen_body);
        if screen_sq.starts_with(&acc_sq) && screen_sq.chars().count() > acc_sq.chars().count() + 20
        {
            return screen_body;
        }
        return acc_body;
    }
    let acc_squashed = squash(&acc_body);
    // A tiny stream fragment ("ok") merely contained somewhere in a
    // longer screen must not summon the whole screen: takeovers need a
    // substantial fragment, matched reflow-proof.
    if screen_body.chars().count() > acc_body.chars().count()
        && (acc_body.is_empty()
            || (acc_squashed.chars().count() >= 8 && squash(&screen_body).contains(&acc_squashed)))
    {
        screen_body
    } else {
        acc_body
    }
}

#[cfg(test)]
#[path = "arbitrate_tests.rs"]
mod tests;
