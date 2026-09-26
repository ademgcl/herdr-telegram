use super::*;

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_final_block_drops_agy_thought_title() {
    // Agy auto titles ("Prioritizing Tool Usage") sit between the `▸ Thought`
    // header and the answer: the title drains, the answer posts alone.
    // Fatal errors after the header still surface; opencode headers (answer
    // directly after) drop nothing — both pinned here.
    let lines = v(&[
        "> build the project",
        "▸ Thought for 11s, 1.5k tokens",
        "Prioritizing Tool Usage",
        "     Build succeeded with 0 errors.",
    ]);
    assert_eq!(
        final_block(&lines, "build the project"),
        v(&["     Build succeeded with 0 errors."])
    );
    let err = "Error from provider (Console): Upstream request failed: boom";
    assert_eq!(
        final_block(&v(&["▸ Thought for 2s", err, "     Retrying."]), "q"),
        v(&[err, "     Retrying."])
    );
    assert_eq!(
        final_block(&v(&["     Thought · 539ms", "     Short answer."]), "q"),
        v(&["     Short answer."])
    );
}

#[test]
fn test_final_block_drops_claude_tool_marker() {
    // D3: Claude ⏺ tool marker splits turns just like ●.
    let lines = v(&[
        "     Preparing changes...",
        "⏺ Write(src/main.rs)",
        "     Applied all modifications.",
    ]);
    assert_eq!(
        final_block(&lines, "apply changes"),
        v(&["     Applied all modifications."])
    );
}
