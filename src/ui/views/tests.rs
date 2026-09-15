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
        ws: "shop".into(),
        cwd: "/tmp/project".into(),
        branch: Some("main".into()),
    };
    let card_with = build_agent_card_text(&with_branch);
    assert!(card_with.contains("branch: 🌿 main"));
    assert!(card_with.contains("status: working"));
    assert!(card_with.contains("cwd: /tmp/project"));

    let without_branch = AgentDetail {
        kind: "opencode".into(),
        pane: "w1:p1".into(),
        title: "backend".into(),
        status: "working".into(),
        ws: "shop".into(),
        cwd: "/tmp/project".into(),
        branch: None,
    };
    let card_without = build_agent_card_text(&without_branch);
    assert!(!card_without.contains("branch:"));
}
