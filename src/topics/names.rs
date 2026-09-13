/// Short, unique, live forum-topic titles: `{emoji} {tag} · {space}`
/// e.g. `🟢 o2 · herdr-telegram`. Tags are stable per pane (persisted) so
/// restarts never reshuffle names.

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

/// Full topic title: `🔄 o2 · herdr-telegram` — emoji status + stable tag.
/// Titles update on genuine transitions only (gated in manager), never per
/// message flicker. NOTE: Telegram accepts `icon_color` solely on topic
/// *creation* — on edit it returns ok:true while changing nothing (verified
/// live), so emoji text is the only working state signal. Don't retry that.
pub fn title(status: &str, tag: &str, space: &str) -> String {
    let short: String = space.chars().take(20).collect();
    format!("{} {tag} · {short}", crate::ui::emoji(status))
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
            "pi", "claude", "codex", "gemini", "cursor", "devin", "agy",
            "cline", "opencode", "copilot", "kimi", "kiro", "droid", "amp",
            "grok", "hermes", "kilo", "qodercli", "qwen", "maki",
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
    fn test_title_format() {
        assert_eq!(title("idle", "o2", "herdr-telegram"), "🟢 o2 · herdr-telegram");
        assert_eq!(title("working", "c1", "ajnow"), "🔄 c1 · ajnow");
        assert_eq!(title("blocked", "a1", "my-project"), "⛔️ a1 · my-project");
        // Long labels are capped at 20 chars.
        assert_eq!(
            title("done", "o1", "a-very-long-workspace-label-here"),
            "✅️ o1 · a-very-long-workspac"
        );
    }
}
