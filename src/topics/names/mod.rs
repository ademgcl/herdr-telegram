//! Stable short tags per pane (`o2`): kept as persisted ids; the
//! VISIBLE title formats in Format B `[ws] label · code` (e.g.
//! `[tg] o2 · o`, shells `[space-1] sh1 · sh`) — see [`format_title`].
//! The topic icon is context-only (agent vs shell,
//! set once at creation); live status surfaces in cards and the typing
//! indicator (plus one unpinned identity card per topic).

mod core;
mod format;

pub use core::topic_core;
pub use format::format_title;

/// 1–2 char code per agent kind. Hand-mapped for all herdr-known agents
/// (single letters collide: claude/cline/copilot/cursor/codex); unknown
/// kinds fall back to their first two alphanumerics.
pub fn code(kind: &str) -> String {
    match kind {
        "opencode" => "o",
        "claude" => "c",
        "codex" => "x",
        "gemini" => "g",
        "agy" => "a",
        "devin" => "d",
        "maki" => "m",
        "hermes" => "h",
        "pi" => "pi",
        "cursor" => "cu",
        "cline" => "cl",
        "copilot" => "co",
        "droid" => "dr",
        "kimi" => "ki",
        "kiro" => "kr",
        "kilo" => "kl",
        "qodercli" => "qo",
        "qwen" => "qw",
        "amp" => "am",
        "grok" => "gr",
        _ => {
            let short: String = kind
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_lowercase();
            if short.is_empty() {
                return "?".into();
            } else {
                return short;
            }
        }
    }
    .to_string()
}

/// Smallest positive `n` such that `{code}{n}` is unused. Deterministic for
/// a given tag set; gaps from closed agents are refilled.
pub fn assign(existing: &[String], kind: &str) -> String {
    let c = code(kind);
    let mut n = 1u32;
    loop {
        let tag = format!("{c}{n}");
        if !existing.iter().any(|t| t == &tag) {
            return tag;
        }
        n += 1;
    }
}

/// Initial topic icon per pane context/kind, set once at topic creation.
/// Never continually mutated on status change.
pub fn context_icon_emoji_id(kind: &str) -> &'static str {
    match kind {
        "shell" | "?" => "5417915203100613993", // 💬 shell prompt
        _ => "5350554349074391003",             // 💻 code agent
    }
}

/// F4: Verify that hardcoded context icon custom emoji IDs exist
/// in the sticker set returned by Telegram's `getForumTopicIconStickers`.
/// Returns any missing emoji IDs (empty if all valid).
pub fn check_context_icons(valid_stickers: &[String]) -> Vec<&'static str> {
    let mut missing = Vec::new();
    let shell_icon = context_icon_emoji_id("shell");
    let agent_icon = context_icon_emoji_id("agent");
    if !valid_stickers.iter().any(|s| s == shell_icon) {
        missing.push(shell_icon);
    }
    if !valid_stickers.iter().any(|s| s == agent_icon) {
        missing.push(agent_icon);
    }
    missing
}

/// F5: Telegram's 6 supported forum topic icon colors in RGB format.
pub const TOPIC_ICON_COLORS: [i64; 6] = [
    0x6FB9F0, // Blue: 7322096
    0xFFD67E, // Yellow: 16766590
    0xCB86DB, // Violet: 13338331
    0x8EEE98, // Green: 9367192
    0xFF93B2, // Pink: 16749490
    0xFB6F5F, // Red: 16478047
];

/// F5: Deterministic mapping from workspace name/label to one of the 6 Telegram icon colors.
pub fn workspace_icon_color(space: &str) -> i64 {
    let clean = space.trim().trim_matches(|c: char| c == '[' || c == ']');
    if clean.is_empty() {
        return TOPIC_ICON_COLORS[0];
    }
    // If space contains or ends with a number (e.g. "ws1", "space-2", "#3"), use numeric index
    let num_suffix = clean
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|digits| digits.parse::<usize>().ok());
    if let Some(n) = num_suffix {
        return TOPIC_ICON_COLORS[n % TOPIC_ICON_COLORS.len()];
    }
    // Deterministic djb2 hash of the clean space string
    let mut hash: usize = 5381;
    for b in clean.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(*b as usize);
    }
    TOPIC_ICON_COLORS[hash % TOPIC_ICON_COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_mapping() {
        assert_eq!(code("opencode"), "o");
        assert_eq!(code("claude"), "c");
        assert_eq!(code("codex"), "x");
        assert_eq!(code("agy"), "a");
        assert_eq!(code("pi"), "pi");
        assert_eq!(code("cursor"), "cu");
        // Unknown kinds: first two alphanumerics, lowercased.
        assert_eq!(code("my-agent_2"), "my");
        assert_eq!(code("Z"), "z");
        assert_eq!(code("???"), "?");
    }

    #[test]
    fn test_codes_unique() {
        let kinds = [
            "pi", "claude", "codex", "gemini", "cursor", "devin", "agy", "cline", "opencode",
            "copilot", "kimi", "kiro", "droid", "amp", "grok", "hermes", "kilo", "qodercli",
            "qwen", "maki",
        ];
        let mut seen = std::collections::HashSet::new();
        for k in kinds {
            assert!(seen.insert(code(k)), "collision on {k}");
        }
    }

    #[test]
    fn test_assign_sequence_and_gap_fill() {
        assert_eq!(assign(&[], "opencode"), "o1");
        let taken = ["o1".to_string(), "o2".to_string(), "c1".to_string()];
        assert_eq!(assign(&taken, "opencode"), "o3");
        assert_eq!(assign(&taken, "claude"), "c2");
        let gapped = ["o1".to_string(), "o3".to_string()];
        assert_eq!(assign(&gapped, "opencode"), "o2");
    }

    #[test]
    fn test_context_icon_mapping() {
        assert_eq!(context_icon_emoji_id("shell"), "5417915203100613993");
        assert_eq!(context_icon_emoji_id("?"), "5417915203100613993");
        assert_eq!(context_icon_emoji_id("opencode"), "5350554349074391003");
    }

    #[test]
    fn test_check_context_icons() {
        let valid = vec![
            "5417915203100613993".to_string(),
            "5350554349074391003".to_string(),
            "1234567890".to_string(),
        ];
        assert!(check_context_icons(&valid).is_empty());

        let missing_agent = vec!["5417915203100613993".to_string()];
        assert_eq!(
            check_context_icons(&missing_agent),
            vec!["5350554349074391003"]
        );

        let none_valid: Vec<String> = vec![];
        assert_eq!(
            check_context_icons(&none_valid),
            vec!["5417915203100613993", "5350554349074391003"]
        );
    }

    #[test]
    fn test_workspace_icon_color_mapping() {
        // Deterministic: bracketed and unbracketed return identical color
        assert_eq!(workspace_icon_color("shop"), workspace_icon_color("[shop]"));
        // Color is always in allowed set
        assert!(TOPIC_ICON_COLORS.contains(&workspace_icon_color("shop")));
        assert!(TOPIC_ICON_COLORS.contains(&workspace_icon_color("")));
        // Numeric suffixes map deterministically to color indices
        assert_eq!(workspace_icon_color("ws0"), TOPIC_ICON_COLORS[0]);
        assert_eq!(workspace_icon_color("ws1"), TOPIC_ICON_COLORS[1]);
        assert_eq!(workspace_icon_color("ws2"), TOPIC_ICON_COLORS[2]);
        assert_eq!(workspace_icon_color("#3"), TOPIC_ICON_COLORS[3]);
    }
}
