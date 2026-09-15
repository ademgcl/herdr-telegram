//! Stable short tags per pane (`o2`): kept as persisted ids; the
//! VISIBLE title syncs 1:1 with herdr pane names (see `sync_title`).
//! The topic icon is context-only (agent vs shell, set once at
//! creation); live status surfaces in cards, pins and typing.

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

/// Friendly default title: `{tag} · {space}` e.g. `o2 · tg`. Written
/// into the herdr pane label when unlabeled, so the default name is a
/// real herdr-tracked name, not just a telegram string.
pub fn title(tag: &str, space: &str) -> String {
    let short: String = space.chars().take(20).collect();
    format!("{tag} · {short}")
}

/// 1:1 pane↔topic title: the herdr pane label, else the unique pane id
/// (`w8:p1`). Blank labels count as unset. Trimmed: the native-rename
/// adopt path trims too, so untrimmed stores would rename every tick.
/// Telegram caps names at 128.
pub fn sync_title(label: Option<&str>, pane: &str) -> String {
    let base = label
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(pane);
    base.chars().take(128).collect()
}

/// Initial topic icon per pane context/kind, set once at topic creation.
/// Never continually mutated on status change.
pub fn context_icon_emoji_id(kind: &str) -> &'static str {
    match kind {
        "shell" | "?" => "5417915203100613993", // 💬 shell prompt
        _ => "5350554349074391003",             // 💻 code agent
    }
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
    fn test_title_format() {
        assert_eq!(title("o2", "tg"), "o2 · tg");
        assert_eq!(title("c1", "ajnow"), "c1 · ajnow");
        // Long labels are capped at 20 chars.
        assert_eq!(
            title("o1", "a-very-long-workspace-label-here"),
            "o1 · a-very-long-workspac"
        );
    }

    #[test]
    fn test_sync_title_label_or_pane_id() {
        assert_eq!(sync_title(Some("api"), "w8:p1"), "api");
        assert_eq!(sync_title(None, "w8:p1"), "w8:p1");
        assert_eq!(sync_title(Some("  "), "w8:p1"), "w8:p1");
        assert_eq!(
            sync_title(Some(&"x".repeat(200)), "w8:p1").chars().count(),
            128
        );
    }
}
