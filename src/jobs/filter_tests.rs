use super::*;

#[test]
fn test_chrome_strips_opencode_tui() {
    assert!(is_chrome("┃  [forum] topic msg for wG:p1: hii"));
    assert!(is_chrome("   Thought · 359ms"));
    assert!(is_chrome("+ Thought: 6.8s"));
    assert!(is_chrome("  ▣  Build · Muse Spark 1.3 Free · 1m 3s"));
    assert!(is_chrome("  Plan · Ling · Zen · high"));
    assert!(is_chrome("╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀"));
    assert!(is_chrome("  ⠋ working…"));
    assert!(is_chrome("→Read src/telegram/router.rs"));
    assert!(is_chrome("← Edit src/jobs/reply.rs"));
    assert!(is_chrome("[watcher] start wG:p1"));
    assert!(is_chrome("[✓] Add session_id to AgentDetail"));
    assert!(is_chrome("# Todos"));
    assert!(is_chrome("Click to expand"));
    assert!(!is_chrome("hello there, how can I help?"));
}

#[test]
fn test_chrome_keeps_prose() {
    // Bare provider words / key hints in normal answers must survive.
    assert!(!is_chrome("refresh tokens expire after an hour"));
    assert!(!is_chrome("I spent an hour on this"));
    assert!(!is_chrome("OpenCode is great for this"));
    assert!(!is_chrome("press ctrl+c to stop"));
    assert!(!is_chrome("Hi! I'm Muse Spark. How can I help today?"));
    // Short indented lines are answers, not padding.
    assert!(!is_chrome("     hi"));
    assert!(!is_chrome("     2"));
    assert!(!is_chrome("     ok"));
    // …but long whitespace-padded art still goes.
    assert!(is_chrome("                              *  *  *"));
}

#[test]
fn test_chrome_strips_agy_tui() {
    assert!(is_chrome(
        "────────────────────────────────────────────────"
    ));
    assert!(is_chrome(">"));
    assert!(is_chrome(
        "? for shortcuts             Gemini 3.8 Flash · high"
    ));
    assert!(is_chrome("Antigravity CLI 1.2.2"));
    assert!(is_chrome("  ADC: firebase-adminsdk-fbsvc@test-proj-123"));
    assert!(is_chrome("○ Bash(cargo test)"));
    assert!(is_chrome("⡿ Running command..."));
    assert!(is_chrome("└ Tip: Run with --nocapture"));
    assert!(is_chrome("▸ Thought for 11s, 1.5k tokens"));
    assert!(is_chrome("  ▸ Thought for 4s"));
    // …but quoted/diff/table content and ASCII rules survive.
    assert!(!is_chrome("> quoted text"));
    assert!(!is_chrome("> added line"));
    assert!(!is_chrome("| a | b |"));
    assert!(!is_chrome("intro --- still content"));
    assert!(!is_chrome("here is a tip for your code"));
    // …and prose mentioning the CLI inline survives (anchored only).
    assert!(!is_chrome("I use Antigravity CLI daily"));
    assert!(!is_chrome("the ADC value rose today"));
}

#[test]
fn test_chrome_keeps_fatal_provider_error_even_framed() {
    let err = "Error from provider (Console): Upstream request failed: [invalid_request_error] reasoning `encrypted_content` was not issued to this caller";
    assert!(!is_chrome(err));
    assert!(!is_chrome(&format!("  ┃  {err}")));
    assert!(!chrome_filtered(&[format!("  ┃  {err}")]).is_empty());
}

#[test]
fn test_chrome_strips_claude_and_codex_chrome() {
    assert!(is_chrome("⏺ Write(src/main.rs)"));
    assert!(is_chrome(
        "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent"
    ));
    assert!(is_chrome("✻ Cooked for 5m 36s · done"));
    assert!(is_chrome("※ recap: completed step 1"));
    assert!(is_chrome("… +4 lines (ctrl + t to view transcript)"));
    assert!(is_chrome("• OpenCode 1.18.31"));
    assert!(is_chrome("Enter to select · Tab/Arrow keys to navigate"));
    assert!(is_chrome(
        "Allow once   Allow always   Reject   ctrl+f fullscreen"
    ));
}

#[test]
fn test_dialog_chrome_keeps_header_arrows() {
    // In dialog mode, ← marks headers and actions (never tool noise).
    assert!(!is_dialog_chrome(
        "← Access external directory ~/.config/opencode"
    ));
    assert!(!is_dialog_chrome("← ☐ Partial ☐ Story ✔ Submit →"));
    // Numbered ❯ options are the dialog body (segment keeps them in the
    // same block) — filtering them here emptied the whole question.
    assert!(!is_dialog_chrome("❯ 1. Allow once"));
    assert!(!is_dialog_chrome("❯ 2) Reject"));
    // Regular chrome still filters.
    assert!(is_dialog_chrome(
        "Enter to select · Tab/Arrow keys to navigate"
    ));
    assert!(is_dialog_chrome("• OpenCode 1.18.31"));
}

#[test]
fn test_chrome_keeps_prose_dash_but_strips_full_rule() {
    // A lone ─ inside prose is content, never a turn separator.
    assert!(!is_chrome("foo ─ bar"));
    assert!(!is_chrome("range 1 ─ 5, see above"));
    // Full-width rules still go (segment parity: >=8 rule chars).
    assert!(is_chrome(
        "────────────────────────────────────────────────"
    ));
    assert!(is_chrome("━━━━━━━━━━━━━━━━"));
}
