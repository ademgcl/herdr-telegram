//! Final-card arbitration: stream vs settled screen. Split from
//! `finalize` (300-line file limit).
use crate::{jobs::segment::final_block, jobs::stream::join_trimmed};

/// Minimum streamed body trusted outright. Below this the stream is
/// assumed starved (alternate-screen TUIs serve chrome-only tails while
/// working) and the settled screen arbitrates.
const STREAM_MIN_CHARS: usize = 40;

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
    let screen_body = join_trimmed(&final_block(screen, prompt));
    // Fatal provider errors settle fast (often before the stream sees
    // them) while `acc` still holds the prior turn. A settled error must
    // never lose to a stale stream — otherwise Telegram repeats the old
    // answer and the error vanishes. When both carry failures the fresh
    // settled screen still wins (it is the current turn).
    let screen_failed = crate::jobs::notices::screen_has_provider_failure(screen)
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
        if screen_sq.starts_with(&acc_sq) && screen_sq.chars().count() > acc_sq.chars().count() + 20 {
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
mod tests {
    use super::*;
    use crate::{types::MAX_MSG_UNITS, ui::chunks};

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// The agy starvation shape (live wC:p2): the stream caught one
    /// wrapped line while the settled screen holds the whole answer.
    fn agy_screen() -> Vec<String> {
        v(&[
            "  • demo-orbit-embedding-fast:",
            "  Working tree is clean on branch",
            "  codex/embedding-fast. All 78",
            "  Jest unit and integration tests",
            "  pass cleanly.",
            "",
            "  Let me know what area you would",
            "  like to focus on next.",
            "",
            "────────────────────────────────────",
            ">",
            "────────────────────────────────────",
            "? for shortcuts             Gemini 3.8 Flash · high",
        ])
    }

    #[test]
    fn test_starved_stream_yields_to_settled_screen() {
        let acc = v(&["  pass cleanly."]);
        let body = select_final_body(&acc, &agy_screen(), "understand the project");
        assert!(
            body.contains("Let me know"),
            "full answer delivered: {body:?}"
        );
        assert!(body.len() > 100);
    }

    #[test]
    fn test_healthy_stream_wins_despite_longer_screen() {
        // Scrollback above the echo must not displace a good stream.
        let acc = v(&["The project has three services, all green and deployed."]);
        let mut screen = v(&["older turn prose from last week that goes on a bit"]);
        screen.extend(agy_screen());
        let body = select_final_body(&acc, &screen, "status?");
        assert_eq!(
            body,
            "The project has three services, all green and deployed."
        );
    }

    #[test]
    fn test_short_genuine_answer_kept() {
        let acc = v(&["ok"]);
        let screen = v(&["  ┃", "  ┃  ping", "     Thought · 100ms", "     ok"]);
        assert_eq!(select_final_body(&acc, &screen, "ping"), "ok");
    }

    #[test]
    fn test_tiny_fragment_never_summons_screen() {
        // Streamed "ok" contained in a much longer settled screen: the
        // short reply stands, the screen does not take over.
        let acc = v(&["ok"]);
        let screen = v(&[
            "older scrollback prose that happens to mention ok in passing and goes on",
            "for quite a while with plenty of extra words beyond the fragment",
        ]);
        assert_eq!(select_final_body(&acc, &screen, "q"), "ok");
    }

    #[test]
    fn test_unrelated_longer_screen_never_displaces_stream() {
        let acc = v(&["first answer here"]);
        let screen = v(&["a completely different and much longer unrelated screen text"]);
        assert_eq!(select_final_body(&acc, &screen, "q"), "first answer here");
    }

    #[test]
    fn test_truncated_stream_merges_settled_tail() {
        // Stream caught the first half (>=40 chars, healthy); the last
        // burst landed after the final stream tick, so the settled
        // screen holds the same answer plus its tail. The tail must not
        // be cut: screen wins via containment + margin.
        let acc = v(&[
            "The migration has three steps. First, back up the database",
            "before running anything else on production.",
        ]);
        let mut screen = acc.clone();
        screen.extend(v(&[
            "Second, run the migrator with --dry-run to preview.",
            "Third, apply and verify row counts match.",
        ]));
        let body = select_final_body(&acc, &screen, "how to migrate?");
        assert!(body.contains("Third, apply"), "tail kept: {body:?}");
        assert!(body.contains("back up the database"), "head kept: {body:?}");
    }

    #[test]
    fn test_head_scrollback_never_displaces_healthy_stream() {
        // Settled screen merged stale scrollback AHEAD of the fresh
        // answer (no boundary split it): contains-match would return
        // the whole merged scrollback as the "reply". Prefix-only
        // takes the tail merge and never leaks the prior turn.
        let acc = v(&[
            "The migration has three steps. First, back up the database",
            "before running anything else on production.",
        ]);
        let mut screen = v(&["older turn prose from last week that goes on a bit"]);
        screen.extend(acc.clone());
        let body = select_final_body(&acc, &screen, "how to migrate?");
        assert!(
            !body.contains("older turn prose"),
            "stale head leaked: {body:?}"
        );
        assert!(body.contains("back up the database"));
    }

    #[test]
    fn test_reflowed_screen_does_not_churn_complete_stream() {
        // Same content, only line-wrap differs (<20 char delta): the
        // stream stands, no churn to the screen copy.
        let acc = v(&["The project has three services, all green and deployed."]);
        let screen = v(&["The project has three services,", "all green and deployed."]);
        assert_eq!(
            select_final_body(&acc, &screen, "status?"),
            "The project has three services, all green and deployed."
        );
    }

    #[test]
    fn test_pipeline_transient_noise_yields_single_final_card() {
        // Finalize-shaped pipeline end to end: streamed deltas + settled
        // screen, both polluted with tool/progress noise → exactly one
        // final-only part. This is the reported ✱/~ leak as a whole.
        let prompt = "fix the guard";
        let answer = "Found it — the guard was missing in check().";
        let acc = v(&[
            "  ┃  fix the guard",
            "     I'll take a look first.",
            "✱ Grep \"guard\" in src (5 matches)",
            "~ Writing command…",
            "     Found it — the guard was missing in check().",
        ]);
        let screen = v(&[
            "     older turn prose that goes on a bit",
            "  ┃  fix the guard",
            "     Thought · 200ms",
            "✱ Grep \"guard\" in src (5 matches)",
            "     Found it — the guard was missing in check().",
            "────────────────────────────────────",
            ">",
            "────────────────────────────────────",
            "? for shortcuts             Gemini 3.8 Flash · high",
        ]);
        let body = select_final_body(&acc, &screen, prompt);
        assert_eq!(body, answer);
        let parts = chunks(&body, MAX_MSG_UNITS);
        assert_eq!(parts.len(), 1, "one final card, never chatter + reply");
        assert_eq!(parts[0], answer);
    }

    #[test]
    fn test_pipeline_transient_only_stream_yields_to_settled_answer() {
        // Starved stream (alt-screen TUI served only noise while working):
        // the transient must not shadow the real answer — the settled
        // screen wins instead of posting chatter.
        let answer = "Found it — the guard was missing in check().";
        let acc = v(&["✱ Grep \"guard\" in src (9 matches)", "~ Writing command…"]);
        let screen = v(&[
            "     Thought · 200ms",
            "✱ Grep \"guard\" in src (9 matches)",
            "     Found it — the guard was missing in check().",
            "────────────────────────────────────",
            ">",
        ]);
        let body = select_final_body(&acc, &screen, "fix the guard");
        assert_eq!(body, answer);
    }

    #[test]
    fn test_empty_both_ways() {
        assert_eq!(select_final_body(&[], &[], "q"), "");
    }

    #[test]
    fn test_settled_provider_error_beats_stale_stream() {
        // Prior turn streamed fine (>=40 chars) but the settled screen is
        // a fast fatal provider failure the stream never saw: the error
        // must win, never a repeat of the old answer.
        let acc = v(&["The project has three services, all green and deployed."]);
        let err = "Error from provider (Console): Upstream request failed: [invalid_request_error] reasoning `encrypted_content` was not issued to this caller";
        let screen = v(&["  ┃", &format!("  ┃  {err}"), "╹▀▀▀▀"]);
        let body = select_final_body(&acc, &screen, "do it");
        assert!(
            body.contains("invalid_request_error"),
            "error surfaced: {body:?}"
        );
        assert!(!body.contains("three services"));
    }

    #[test]
    fn test_fresh_error_beats_stale_error() {
        // Both turns failed: the fresh settled screen (current turn) wins
        // over the prior turn's streamed error.
        let old_err = "Error from provider (Console): old rate limit, retry later";
        let new_err = "Error from provider (Console): Upstream request failed: [invalid_request_error] fresh failure";
        let acc = v(&[&format!("  ┃  {old_err}")]);
        let screen = v(&["  ┃", &format!("  ┃  {new_err}"), "╹▀▀▀▀"]);
        let body = select_final_body(&acc, &screen, "do it");
        assert!(body.contains("fresh failure"), "fresh wins: {body:?}");
        assert!(!body.contains("old rate limit"));
    }
}
