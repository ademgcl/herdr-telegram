//! Stable short tags per pane (`o2`): kept as persisted ids; the
//! VISIBLE title formats in Format B `[ws] label · agent` (see `format_title`).
//! The topic icon is context-only (agent vs shell, set once at
//! creation); live status surfaces in cards and the typing indicator
//! (+ one identity pin per topic).

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

/// Format B topic title: `[{space}] {label_or_tag} · {agent}`.
/// e.g. `[shop] shop-backend · claude` (when labeled),
/// or `[tg] o2 · opencode` (when unlabeled, using assigned tag).
///
/// Idempotent: if `label_or_tag` already begins with `[` (custom or previously
/// formatted title), it preserves it as-is without double-wrapping brackets.
/// Strips redundant trailing ` · {agent}` or ` · {space}` suffixes.
///
/// Telegram caps topic names at 128 UTF-8 characters; space is truncated to 20.
pub fn format_title(space: &str, label_or_tag: &str, kind: &str) -> String {
    let trimmed = label_or_tag.trim();
    if trimmed.starts_with('[') {
        return trimmed.chars().take(128).collect();
    }

    let raw_space = space.trim();
    let short_space: String = if raw_space.is_empty() || raw_space == "?" {
        "ws".to_string()
    } else {
        raw_space.chars().take(20).collect()
    };

    let clean_agent = if kind.trim().is_empty() || kind.trim() == "?" {
        "agent"
    } else {
        kind.trim()
    };

    let mut body = trimmed;
    let agent_suffix = format!(" · {clean_agent}");
    if let Some(stripped) = body.strip_suffix(&agent_suffix) {
        body = stripped.trim();
    }
    let space_suffix = format!(" · {short_space}");
    if let Some(stripped) = body.strip_suffix(&space_suffix) {
        body = stripped.trim();
    }

    let formatted = format!("[{short_space}] {body} · {clean_agent}");
    formatted.chars().take(128).collect()
}

/// Friendly default title in Format B: `[{space}] {tag} · {kind}` e.g. `[tg] o2 · opencode`.
/// Written into the herdr pane label when unlabeled, so the default name is a
/// real herdr-tracked name, not just a telegram string.
pub fn title(tag: &str, space: &str, kind: &str) -> String {
    format_title(space, tag, kind)
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
        assert_eq!(title("o2", "tg", "opencode"), "[tg] o2 · opencode");
        assert_eq!(title("c1", "ajnow", "claude"), "[ajnow] c1 · claude");
        assert_eq!(
            title("o1", "a-very-long-workspace-label-here", "opencode"),
            "[a-very-long-workspac] o1 · opencode"
        );
        assert_eq!(
            format_title("shop", "shop-backend", "claude"),
            "[shop] shop-backend · claude"
        );
        assert_eq!(
            format_title("shop", "[shop] shop-backend · claude", "claude"),
            "[shop] shop-backend · claude"
        );
        assert_eq!(
            format_title("tg", "o2 · tg", "opencode"),
            "[tg] o2 · opencode"
        );
        assert_eq!(format_title("infra", "s1", "shell"), "[infra] s1 · shell");
    }
}
