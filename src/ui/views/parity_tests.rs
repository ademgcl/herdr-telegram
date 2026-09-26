use super::*;

#[test]
fn test_dm_help_lists_every_dm_command() {
    // DM router (dm.rs) handles each of these — help must not hide one.
    // (/start + /help are escape hatches, listed nowhere by design.)
    let help = help_text();
    for cmd in [
        "/agents",
        "/spawn",
        "/space",
        "/model",
        "/quit",
        "/kill",
        "/shell",
        "/pane",
        "/split",
        "/read",
        "/output",
        "/card",
        "/esc",
        "/status",
        "/history",
        "/reset",
        "/cancel",
        "/keys",
        "/transient",
    ] {
        assert!(help.contains(cmd), "DM help missing {cmd}");
    }
    assert!(help.contains("[space]"), "DM help must use [space]");
    assert!(
        !help.contains("[workspace]"),
        "DM help must not use [workspace]"
    );
}

/// Backtick-fenced `/commands` inside a help text — the documented set
/// for bidirectional Help≡router checks (first token only, so
/// "`/read [n]`" counts as /read).
fn fenced_cmds(help: &str) -> std::collections::HashSet<String> {
    help.split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|s| s.split_whitespace().next())
        .filter(|w| w.starts_with('/'))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '_')
                .to_string()
        })
        .collect()
}

/// Both directions with one explicit exemption: every router arm
/// documented (no silent commands), nothing documented without an arm
/// (no dead ends). `/space` is an explicit arm everywhere: a real arm in
/// DM (`dm.rs`), a pre-dispatch intercept in forums (`forum.rs`) that
/// each surface's arm list must name — no implicit inject, or a broken
/// intercept would still pass. (/start+/help are escape hatches —
/// skipped in arm lists where unlisted by design.)
fn assert_parity(help: &str, arms: &[&str], surface: &str) {
    let documented = fenced_cmds(help);
    let armed: std::collections::HashSet<String> = arms.iter().map(|s| s.to_string()).collect();
    for cmd in &armed {
        assert!(documented.contains(cmd), "{surface} help hides {cmd}");
    }
    for cmd in documented {
        assert!(armed.contains(&cmd), "{surface} help lists unarmed {cmd}");
    }
}

#[test]
fn test_topic_help_matches_topic_router() {
    // Arms in forum_topic.rs (+ /space forum.rs intercept, named
    // explicitly; /help never lists itself — AGENTS.md-exempt).
    let help = topic_help_text("w1:p1", "opencode");
    assert_parity(
        &help,
        &[
            "/start",
            "/agents",
            "/spawn",
            "/space",
            "/reset",
            "/cancel",
            "/card",
            "/esc",
            "/quit",
            "/kill",
            "/split",
            "/pane",
            "/shell",
            "/read",
            "/output",
            "/history",
            "/keys",
            "/status",
            "/model",
            "/transient",
        ],
        "topic",
    );
}

#[test]
fn test_shell_help_matches_shell_router() {
    // Arms in shell_topic.rs (+ /space intercept, named; refuses count).
    let help = shell_help_text("w1:p1");
    assert_parity(
        &help,
        &[
            "/start",
            "/agents",
            "/spawn",
            "/space",
            "/shell",
            "/reset",
            "/quit",
            "/kill",
            "/split",
            "/pane",
            "/cancel",
            "/card",
            "/esc",
            "/read",
            "/output",
            "/history",
            "/keys",
            "/status",
            "/model",
            "/transient",
        ],
        "shell",
    );
}

#[test]
fn test_general_help_matches_general_router() {
    // Arms in general.rs (+ /space intercept, named; redirect arms count).
    // (/start + /help unlisted by design — escape hatches.)
    assert_parity(
        &general_help_text(),
        &[
            "/agents",
            "/spawn",
            "/space",
            "/cancel",
            "/card",
            "/esc",
            "/model",
            "/history",
            "/shell",
            "/pane",
            "/reset",
            "/quit",
            "/kill",
            "/split",
            "/read",
            "/output",
            "/status",
            "/keys",
            "/transient",
        ],
        "general",
    );
}

#[test]
fn test_dm_help_matches_dm_router() {
    // Arms in dm.rs (+ /space arm, listed like the rest here).
    assert_parity(
        help_text(),
        &[
            "/agents",
            "/spawn",
            "/space",
            "/model",
            "/quit",
            "/kill",
            "/shell",
            "/pane",
            "/split",
            "/read",
            "/output",
            "/card",
            "/esc",
            "/status",
            "/history",
            "/reset",
            "/cancel",
            "/keys",
            "/transient",
        ],
        "dm",
    );
}
