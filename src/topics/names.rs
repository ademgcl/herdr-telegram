/// Short, unique forum-topic titles: `{tag} · {space}` e.g.
/// `o2 · herdr-telegram`. Tags are stable per pane (persisted) so
/// restarts never reshuffle names. State lives on the topic ICON.

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

/// Short, unique forum-topic titles: `{tag} · {space}` e.g.
/// `o2 · herdr-telegram`. Pure text, set once — state lives on the topic
/// ICON (see `icon_emoji_id`), never in the title.

/// Quiet horizon: a `done` older than this displays as idle (available
/// again). herdr parks agents at done indefinitely, so without decay the
/// idle badge would almost never show.
pub const DONE_IDLE_AFTER_SECS: u64 = 15 * 60;

/// Display status for the icon: fresh completions show done; long-quiet
/// ones relax to idle. herdr-truth (`done`) is preserved everywhere else —
/// only the badge decays.
pub fn display_status(status: &str, settled_secs: u64) -> &str {
    if status == "done" && settled_secs >= DONE_IDLE_AFTER_SECS {
        "idle"
    } else {
        status
    }
}
pub fn title(tag: &str, space: &str) -> String {
    let short: String = space.chars().take(20).collect();
    format!("{tag} · {short}")
}

/// Topic icon per agent status, as preset custom-emoji IDs (verified live:
/// unlike `icon_color`, `icon_custom_emoji_id` applies AND renders on
/// edit). Instant, silent, zero clutter — the at-a-glance state signal.
pub fn icon_emoji_id(status: &str) -> &'static str {
    match status {
        "working" => "5350554349074391003", // 💻
        "idle" => "5350392020785437399",    // ☕️
        "done" => "5237699328843200968",    // ✅
        "blocked" => "5379748062124056162", // ❗️
        "closed" | "dead" | "exited" => "5408906741125490282", // 🏁
        _ => "5377316857231450742",         // ❓
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
        assert_eq!(title("o2", "herdr-telegram"), "o2 · herdr-telegram");
        assert_eq!(title("c1", "ajnow"), "c1 · ajnow");
        // Long labels are capped at 20 chars.
        assert_eq!(
            title("o1", "a-very-long-workspace-label-here"),
            "o1 · a-very-long-workspac"
        );
    }
    #[test]
    fn test_icon_mapping() {
        assert_eq!(icon_emoji_id("working"), "5350554349074391003");
        assert_eq!(icon_emoji_id("idle"), "5350392020785437399");
        assert_eq!(icon_emoji_id("done"), "5237699328843200968");
        assert_eq!(icon_emoji_id("blocked"), "5379748062124056162");
        assert_eq!(icon_emoji_id("exited"), "5408906741125490282");
        assert_eq!(icon_emoji_id("whatever"), "5377316857231450742");
    }

    #[test]
    fn test_display_status_decay() {
        assert_eq!(display_status("done", 0), "done");
        assert_eq!(display_status("done", 14 * 60), "done");
        assert_eq!(display_status("done", 15 * 60), "idle");
        assert_eq!(display_status("done", 3600), "idle");
        // Only done decays — everything else passes through.
        assert_eq!(display_status("working", 3600), "working");
        assert_eq!(display_status("blocked", 3600), "blocked");
        assert_eq!(display_status("idle", 0), "idle");
    }
}
