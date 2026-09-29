use super::*;

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_idle_screen_yields_answer_not_footer() {
    // A settled screen (frame + Thought header + answer + step footer
    // + idle footer) reduces to the answer — never status-bar soup.
    let screen = v(&[
        "  ┃",
        "  ┃  hi",
        "  ┃",
        "     Thought · 539ms",
        "     Hi! I'm Muse Spark. How can I help with herdr-telegram today?",
        "     ▣  Build · Muse Spark 1.3 Free · 5.1s",
        "  ┃",
        "  ┃  Build · Muse Spark 1.3 Free OpenCode Zen · xhigh",
        "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
        "   /home/user/projects/herdr-telegram        10.8K (1%)  ctrl+p commands",
    ]);
    assert_eq!(
        final_block(&screen, "hi"),
        v(&["     Hi! I'm Muse Spark. How can I help with herdr-telegram today?"])
    );
}

#[test]
fn test_final_block_drops_work_before_tool_call() {
    let lines = v(&[
        "     I'll take a look first.",
        "→Read src/main.rs",
        "     Found it — the guard is missing.",
    ]);
    assert_eq!(
        final_block(&lines, "fix it"),
        v(&["     Found it — the guard is missing."])
    );
}

#[test]
fn test_final_block_splits_multi_turn_scrollback() {
    // Earlier turns above the prompt echo are dropped; only the fresh
    // reply survives.
    let lines = v(&[
        "     Hi — this is herdr-telegram.",
        "     You've got 4 modified files.",
        "  ┃",
        "  ┃  hi",
        "  ┃",
        "     Hey — what do you need in herdr-telegram?",
    ]);
    assert_eq!(
        final_block(&lines, "hi"),
        v(&["     Hey — what do you need in herdr-telegram?"])
    );
}

#[test]
fn test_final_block_keeps_unframed_prompt_repeat() {
    // An answer line repeating the prompt is content, not an echo:
    // only the framed echo splits.
    let lines = v(&["  ┃  hi", "     hi", "     How can I help?"]);
    assert_eq!(
        final_block(&lines, "hi"),
        v(&["     hi", "     How can I help?"])
    );
}

#[test]
fn test_final_block_no_boundaries_returns_all() {
    let lines = v(&["first line", "second line"]);
    assert_eq!(final_block(&lines, "hi"), lines);
}

#[test]
fn test_ascii_rule_and_quotes_never_split() {
    // Markdown `---`, blockquotes, diff and table lines are content.
    let lines = v(&["intro", "---", "> quoted", "> added", "| a | b |", "end"]);
    assert_eq!(final_block(&lines, "q"), lines);
}

#[test]
fn test_permission_dialog_keeps_question_and_options() {
    // Real opencode permission dialog as herdr reports it: everything
    // framed. De-framed, path + options survive the pipeline while
    // footers drop (see waiting_text).
    use crate::jobs::filter::deframe;
    let raw = v(&[
        "  ┃",
        "  ┃  △ Permission required",
        "  ┃    ← Access external directory ~/.config/opencode",
        "  ┃",
        "  ┃  Patterns",
        "  ┃",
        "  ┃  - /home/user/.config/opencode/*",
        "  ┃",
        "  ┃",
        "  ┃   Allow once   Allow always   Reject",
        "  ┃",
        "  ┃  ctrl+f fullscreen  ⇆ select  enter confirm",
        "  ┃",
    ]);
    assert_eq!(
        deframe("  ┃  △ Permission required"),
        "△ Permission required"
    );
    assert_eq!(deframe("plain line"), "plain line");
    let lines: Vec<String> = raw.iter().map(|l| deframe(l)).collect();
    let out = final_block(&lines, "");
    let body = out.join("\n");
    assert!(body.contains("Allow once"), "options kept: {body}");
    assert!(body.contains("opencode/*"), "path kept: {body}");
    assert!(!body.contains("ctrl+p"), "footer dropped: {body}");
}

#[test]
fn test_final_block_drops_agy_tool_noise() {
    let lines = v(&[
        "> build the project",
        "▸ Thought for 11s, 1.5k tokens",
        "● Edit(src/main.rs)",
        "○ Bash(cargo test)",
        "⡿ Running command...",
        "└ Tip: Use --release for faster builds",
        "────────────────────────────────────────────────",
        ">",
        "? for shortcuts             Gemini 3.8 Flash · high",
    ]);
    assert_eq!(
        final_block(&lines, "build the project"),
        Vec::<String>::new()
    );

    let lines_with_answer = v(&[
        "> build the project",
        "▸ Thought for 11s, 1.5k tokens",
        "● Edit(src/main.rs)",
        "○ Bash(cargo test)",
        "     Build succeeded with 0 errors.",
        "────────────────────────────────────────────────",
        ">",
    ]);
    assert_eq!(
        final_block(&lines_with_answer, "build the project"),
        v(&["     Build succeeded with 0 errors."])
    );

    let lines_thinking = v(&["> build the project", "▸ Thought for 11s, 1.5k tokens"]);
    assert_eq!(
        final_block(&lines_thinking, "build the project"),
        Vec::<String>::new()
    );
}

#[test]
fn test_final_block_claude_status_bar_fallback() {
    // B1: Claude status bar (⏵⏵ auto mode on …) must never deliver as reply card.
    let screen_with_answer = v(&[
        "     I refactored the module.",
        "────────────────────────────────────────────────",
        "❯",
        "────────────────────────────────────────────────",
        "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent",
    ]);
    assert_eq!(
        final_block(&screen_with_answer, ""),
        v(&["     I refactored the module."])
    );

    let idle_screen = v(&[
        "❯",
        "────────────────────────────────────────────────",
        "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent",
    ]);
    assert_eq!(final_block(&idle_screen, ""), Vec::<String>::new());
}

#[test]
fn test_final_block_codex_footer_fallback() {
    // C1: Codex model/cwd footer must never deliver as reply card.
    let screen_with_answer = v(&[
        "     All tests passed successfully.",
        "› Ask Codex to do anything",
        "gpt-5.6-sol medium · ~/develop/ws/omnitrack · Resume product brief",
    ]);
    assert_eq!(
        final_block(&screen_with_answer, ""),
        v(&["     All tests passed successfully."])
    );

    let idle_screen = v(&[
        "› Ask Codex to do anything",
        "gpt-5.6-sol medium · ~/develop/ws/omnitrack · Resume product brief",
    ]);
    assert_eq!(final_block(&idle_screen, ""), Vec::<String>::new());
}

#[test]
fn test_final_block_prompted_codex_footer_never_wins() {
    // Watcher finals pass the prompt (`want` non-empty): the footer
    // pop/drain loop must run there too — gated to the spontaneous path
    // it posted the model footer as the reply and dropped the answer.
    let screen_with_answer = v(&[
        "     All tests passed successfully.",
        "› Ask Codex to do anything",
        "gpt-5.6-sol medium · ~/develop/ws/omnitrack · Resume product brief",
    ]);
    assert_eq!(
        final_block(&screen_with_answer, "run tests"),
        v(&["     All tests passed successfully."])
    );
    // Prompted idle screen still yields nothing (footer-only tail pops
    // all the way out instead of delivering the footer).
    let idle_screen = v(&[
        "› Ask Codex to do anything",
        "gpt-5.6-sol medium · ~/develop/ws/omnitrack · Resume product brief",
    ]);
    assert_eq!(final_block(&idle_screen, "run tests"), Vec::<String>::new());
}

#[test]
fn test_transient_inventory_never_posts_answer_always_wins() {
    use crate::jobs::{
        filter::is_chrome,
        transient::{PROSE, TRANSIENT},
    };
    // Invariant: every inventoried shape is handled (chrome or boundary).
    // A future TUI addition fails here until filtered — never as a card.
    for &t in TRANSIENT {
        assert!(
            is_chrome(t) || is_boundary(t),
            "unhandled transient shape: {t}"
        );
    }
    // Matrix, prompted + spontaneous: alone → nothing posts (the watcher
    // keeps waiting for the real final); flanking the answer → answer only,
    // trailing noise never shadows it.
    let answer = "     The guard was missing — fixed and tested.";
    for &t in TRANSIENT {
        assert_eq!(
            final_block(&v(&[t]), "fix it"),
            Vec::<String>::new(),
            "alone posts: {t}"
        );
        assert_eq!(
            final_block(&v(&[t]), ""),
            Vec::<String>::new(),
            "spontaneous posts: {t}"
        );
        assert_eq!(
            final_block(&v(&[t, answer]), "fix it"),
            v(&[answer]),
            "before leaks: {t}"
        );
        assert_eq!(
            final_block(&v(&[answer, t]), "fix it"),
            v(&[answer]),
            "after shadows: {t}"
        );
        assert_eq!(
            final_block(&v(&[answer, t]), ""),
            v(&[answer]),
            "spontaneous shadows: {t}"
        );
    }
    // Prose battery survives intact on both paths.
    for &p in PROSE {
        assert_eq!(final_block(&v(&[p]), "fix it"), v(&[p]), "prose lost: {p}");
        assert_eq!(
            final_block(&v(&[p]), ""),
            v(&[p]),
            "spontaneous prose lost: {p}"
        );
    }
}
