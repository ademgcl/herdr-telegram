//! Proc tests. Split from `proc` (300-line file limit).
use super::*;

#[test]
fn test_looks_like_bot_exact() {
    // Bare daemons match; every subcommand form refuses.
    assert!(looks_like_bot("/Users/x/t/target/release/herdr-telegram"));
    assert!(looks_like_bot("./target/debug/herdr-telegram"));
    assert!(!looks_like_bot(
        "/Users/x/t/target/debug/herdr-telegram ctl trigger w1:p1 blocked"
    ));
    assert!(!looks_like_bot(
        "/Users/x/t/target/debug/herdr-telegram dev"
    ));
    assert!(!looks_like_bot(
        "/Users/x/t/target/debug/herdr-telegram dev status"
    ));
    assert!(!looks_like_bot(
        "/Users/x/t/target/debug/herdr-telegram ops cleanup"
    ));
    // Checkout path containing `ops` must not wedge cleanup.
    assert!(looks_like_bot(
        "/Users/x/ops/herdr-telegram/target/release/herdr-telegram"
    ));
    // Unrelated processes never match.
    assert!(!looks_like_bot("pgrep -f target/release/herdr-telegram"));
    assert!(!looks_like_bot("/usr/bin/some-daemon"));
    assert!(!looks_like_bot(""));
    // Install paths with spaces still match the bare daemon…
    assert!(looks_like_bot("/Users/a b/tools/herdr-telegram"));
    // …but never a pgrep line or a subcommand.
    assert!(!looks_like_bot("pgrep -f /Users/a b/herdr-telegram"));
    assert!(!looks_like_bot("/Users/a b/herdr-telegram ctl"));
}
