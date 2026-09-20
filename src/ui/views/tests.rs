use super::*;

#[test]
fn test_fit_short_text() {
    let short = "Hello World";
    assert_eq!(fit(short, 100), short);
}

#[test]
fn test_fit_truncation() {
    let long = "A".repeat(100);
    let fitted = fit(&long, 40);
    assert!(fitted.contains("… [truncated] …"));
    assert!(fitted.encode_utf16().count() <= 40);
}

#[test]
fn test_fit_non_ascii_no_panic() {
    let s = format!("{}{}", "\u{1F600}".repeat(40), "\u{4E2D}".repeat(40));
    let fitted = fit(&s, 40);
    assert!(fitted.encode_utf16().count() <= 40);
    assert!(fitted.contains("… [truncated] …"));
}

#[test]
fn test_topic_help_text() {
    let help = topic_help_text("w1:p1", "claude");
    assert!(help.contains("claude"));
    assert!(help.contains("w1:p1"));
    let shell = shell_help_text("w1:p1");
    assert!(shell.contains("w1:p1"));
    assert!(shell.contains("opencode"));
    assert!(shell.contains("claude"));
    assert!(shell.contains("re-enter"));
}

#[test]
fn test_dm_help_lists_every_dm_command() {
    // DM router (dm.rs) handles each of these — help must not hide one.
    // (/start + /help are escape hatches, listed nowhere by design.)
    let help = help_text();
    for cmd in [
        "/agents", "/spawn", "/space", "/model", "/quit", "/kill", "/shell", "/pane", "/split",
        "/read", "/output", "/card", "/esc", "/status", "/history", "/reset", "/cancel", "/keys",
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
            "/start", "/agents", "/spawn", "/space", "/reset", "/cancel", "/card", "/esc", "/quit",
            "/kill", "/split", "/pane", "/shell", "/read", "/output", "/history", "/keys",
            "/status", "/model",
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
            "/start", "/agents", "/spawn", "/space", "/shell", "/reset", "/quit", "/kill",
            "/split", "/pane", "/cancel", "/card", "/esc", "/read", "/output", "/history", "/keys",
            "/status", "/model",
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
            "/agents", "/spawn", "/space", "/cancel", "/card", "/esc", "/model", "/history",
            "/shell", "/pane", "/reset", "/quit", "/kill", "/split", "/read", "/output", "/status",
            "/keys",
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
            "/agents", "/spawn", "/space", "/model", "/quit", "/kill", "/shell", "/pane", "/split",
            "/read", "/output", "/card", "/esc", "/status", "/history", "/reset", "/cancel",
            "/keys",
        ],
        "dm",
    );
}

#[test]
fn test_ws_label() {
    let spaces = vec![
        WorkspaceInfo {
            id: "w1".to_string(),
            label: "shop".to_string(),
            number: 1,
        },
        WorkspaceInfo {
            id: "w2".to_string(),
            label: "  ".to_string(),
            number: 2,
        },
    ];
    assert_eq!(ws_label(&spaces, "w1"), "shop");
    assert_eq!(ws_label(&spaces, "w2"), "w2");
    assert_eq!(ws_label(&spaces, "w3"), "w3");
}

#[test]
fn test_build_agent_card_text_with_and_without_branch() {
    let with_branch = AgentDetail {
        kind: "opencode".into(),
        pane: "w1:p1".into(),
        title: "backend".into(),
        status: "working".into(),
        ws: "w8".into(),
        cwd: "/tmp/project".into(),
        branch: Some("main".into()),
    };
    let card_with = build_agent_card_text(&with_branch, "shop");
    assert!(card_with.contains("branch: 🌿 main"));
    assert!(card_with.contains("status: working"));
    assert!(card_with.contains("cwd: /tmp/project"));
    assert!(card_with.contains("space: shop"));
    assert!(!card_with.contains("w8"));

    let without_branch = AgentDetail {
        kind: "opencode".into(),
        pane: "w1:p1".into(),
        title: "backend".into(),
        status: "working".into(),
        ws: "w8".into(),
        cwd: "/tmp/project".into(),
        branch: None,
    };
    let card_without = build_agent_card_text(&without_branch, "shop");
    assert!(!card_without.contains("branch:"));
}

#[test]
fn test_build_identity_card_text() {
    let full = build_identity_card_text(
        "claude",
        "w1:p2",
        "shop",
        "working",
        Some("auth-service"),
        Some("feature/login"),
    );
    assert!(full.contains("📌 claude · w1:p2"));
    assert!(full.contains("Workspace: shop"));
    assert!(full.contains("Title: auth-service"));
    assert!(full.contains("Branch: 🌿 feature/login"));
    assert!(full.contains("Status: 🔄 working"));

    let minimal = build_identity_card_text("shell", "w1:p3", "infra", "idle", None, None);
    assert!(minimal.contains("📌 shell · w1:p3"));
    assert!(minimal.contains("Workspace: infra"));
    assert!(!minimal.contains("Title:"));
    assert!(!minimal.contains("Branch:"));
    assert!(minimal.contains("Status: 🟢 ready"));
}

#[test]
fn test_chunks_empty_yields_no_message() {
    // Empty bodies must not produce a `[""]` chunk (Telegram 400s it).
    assert!(chunks("", 100).is_empty());
    assert!(!chunks("hi", 100).is_empty());
}
