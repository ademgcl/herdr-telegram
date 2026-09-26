//! $HOME-mask tests for menu/ws/agent/identity text (split from
//! `views/tests`, 300-line file limit).
use super::*;

#[test]
fn test_menu_text_masks_home_paths() {
    // Space labels / kinds carrying terminal paths must not leak $HOME
    // to the chat (agent-card parity). Uses the real home so mask_home
    // actually fires.
    use crate::types::{AgentRow, WorkspaceInfo, home_dir, mask_home_with};
    let home = home_dir();
    assert!(!home.is_empty(), "test needs a HOME to mask");
    let spaces = vec![WorkspaceInfo {
        id: "w8".into(),
        label: format!("{home}/shop"),
        number: 8,
    }];
    let agents = vec![AgentRow {
        kind: format!("{home}/opencode"),
        pane: "w8:p1".into(),
        title: String::new(),
        status: "working".into(),
        ws: "w8".into(),
    }];
    let text = build_menu_text(&spaces, &agents);
    assert_eq!(
        text,
        mask_home_with(&text, &home),
        "menu text must carry no raw $HOME"
    );
    assert!(!text.contains(&*home));
}

#[test]
fn test_ws_text_masks_home_paths() {
    // build_ws_text parity with build_menu_text: space labels carry
    // terminal paths ($HOME/username) shown to the whole chat.
    use crate::types::{WorkspaceInfo, home_dir};
    let home = home_dir();
    assert!(!home.is_empty(), "test needs a HOME to mask");
    let spaces = vec![WorkspaceInfo {
        id: "w8".into(),
        label: format!("{home}/shop"),
        number: 8,
    }];
    let text = build_ws_text("w8", &spaces, &[]);
    assert!(!text.contains(&*home), "ws text must carry no raw $HOME");
    assert!(text.contains("~"), "masked home should render as ~");
}

#[test]
fn test_agent_and_identity_cards_mask_space_paths() {
    // Agent + identity card parity with build_ws_text: the space label
    // rides both cards to the chat, never raw $HOME.
    use crate::types::{AgentDetail, home_dir};
    let home = home_dir();
    assert!(!home.is_empty(), "test needs a HOME to mask");
    let space = format!("{home}/shop");
    let a = AgentDetail {
        pane: "w8:p1".into(),
        kind: "opencode".into(),
        ws: "w8".into(),
        cwd: "/tmp".into(),
        title: "t".into(),
        branch: None,
        status: "working".into(),
    };
    assert!(!build_agent_card_text(&a, &space).contains(&*home));
    let id = build_identity_card_text("opencode", "w8:p1", &space, "working", None, None);
    assert!(!id.contains(&*home));
}
