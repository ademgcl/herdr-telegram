//! Stable short tags per pane (`o2`): kept as persisted ids; the
//! VISIBLE title formats in Format B `[ws] label · agent` (shells bare:
//! `[ws] label`) — see [`format_title`]. The topic icon is context-only (agent vs shell,
//! set once at creation); live status surfaces in cards and the typing
//! indicator (plus one unpinned identity card per topic).

mod format;

pub use format::{format_title, title};

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
}
