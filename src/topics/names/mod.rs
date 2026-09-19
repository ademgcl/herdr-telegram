//! Stable short tags per pane (`o2`): kept as persisted ids; the
//! VISIBLE title is bare Format B `[ws] label` (e.g. `[tg] o2`) — see
//! [`format_title`]. Kind lives in the topic icon only (one glyph per
//! agent, 💬 shell); live status surfaces in cards and the typing
//! indicator (plus one unpinned identity card per topic).

mod chrome;
mod core;
mod format;

pub(crate) use chrome::{norm_title, space_rename_parts};
pub use core::topic_core;
pub use format::format_title;

#[cfg(test)]
#[path = "icon_tests.rs"]
mod icon_tests;

/// Full-name → short-code map for every herdr-known agent kind, single
/// source for [`code`] and title-chrome shedding.
/// Single letters collide, so claude/cline/copilot/cursor/codex are
/// hand-disambiguated; `shell` is explicit (code `sh`).
pub(crate) const KIND_CODES: &[(&str, &str)] = &[
    ("opencode", "o"),
    ("claude", "c"),
    ("codex", "x"),
    ("gemini", "g"),
    ("agy", "a"),
    ("devin", "d"),
    ("maki", "m"),
    ("hermes", "h"),
    ("pi", "pi"),
    ("cursor", "cu"),
    ("cline", "cl"),
    ("copilot", "co"),
    ("droid", "dr"),
    ("kimi", "ki"),
    ("kiro", "kr"),
    ("kilo", "kl"),
    ("qodercli", "qo"),
    ("qwen", "qw"),
    ("amp", "am"),
    ("grok", "gr"),
    ("shell", "sh"),
];

/// 1–2 char code per agent kind ([`KIND_CODES`]); unknown kinds fall back
/// to their first two alphanumerics. Case-blind: herdr kinds are
/// lowercase but callers may pass `SHELL`/`Opencode`.
pub fn code(kind: &str) -> String {
    let k = kind.trim().to_lowercase();
    if let Some((_, short)) = KIND_CODES.iter().find(|(name, _)| *name == k) {
        return short.to_string();
    }
    let short: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_lowercase();
    if short.is_empty() {
        return "?".into();
    }
    short
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

/// One topic icon per mapped kind + generic fallback (custom emoji IDs from Telegram's
/// fixed forum-topic set — no logos exist, so glyphs are associative:
/// 💻 code, 💬 shell prompt, 🤖 bot, 🧠 big brain, 📝 writing, 🔮 crystal
/// ball, 👀 watching). `shell` keeps 💬 forever: the shell detector
/// cross-checks it. Unknown kinds fall back to 🤖 (never 💬, so unknowns
/// can't read as shells). Case-blind like [`code`].
const KIND_ICONS: &[(&str, &str)] = &[
    ("opencode", "5350554349074391003"), // 💻
    ("shell", "5417915203100613993"),    // 💬
    ("agy", "5309832892262654231"),      // 🤖
    ("claude", "5237889595894414384"),   // 🧠
    ("codex", "5373251851074415873"),    // 📝
    ("gemini", "5350367161514732241"),   // 🔮
    ("cursor", "5357121491508928442"),   // 👀
];

/// Generic agent glyph for unmapped kinds.
const FALLBACK_ICON: &str = "5309832892262654231"; // 🤖

/// Topic icon for a pane kind: table hit or the generic bot glyph.
/// Never mutates on status change — only kind flips re-icon (see
/// `icon_needs_update`), and user-customized icons are never touched.
pub fn context_icon_emoji_id(kind: &str) -> &'static str {
    let k = kind.trim().to_lowercase();
    KIND_ICONS
        .iter()
        .find(|(name, _)| *name == k)
        .map(|(_, id)| *id)
        .unwrap_or(FALLBACK_ICON)
}

/// True when the icon id is one of ours (table + fallback): a stored id
/// outside this set is user-customized and must never be overwritten.
pub fn is_bot_icon(id: &str) -> bool {
    FALLBACK_ICON == id || KIND_ICONS.iter().any(|(_, known)| *known == id)
}

/// Icon to write when the pane's kind is `kind` and the stored icon is
/// `stored`: `Some` for bot-owned stale icons (kind flip since the last
/// write) AND for missing icons with a known kind (a create-time icon
/// RPC failure must heal on the next tick, not keep the default glyph
/// forever) — `None` covers current, `?`/empty kinds, and user customs.
pub fn icon_needs_update(stored: Option<&str>, kind: &str) -> Option<&'static str> {
    let k = kind.trim().to_lowercase();
    if k.is_empty() || k == "?" {
        return None;
    }
    let want = context_icon_emoji_id(&k);
    match stored {
        None => Some(want),
        Some(cur) if is_bot_icon(cur) && cur != want => Some(want),
        _ => None,
    }
}

/// F4: Verify that hardcoded icon custom emoji IDs exist
/// in the sticker set returned by Telegram's `getForumTopicIconStickers`.
/// Returns any missing emoji IDs (empty if all valid).
pub fn check_context_icons(valid_stickers: &[String]) -> Vec<&'static str> {
    let mut missing = Vec::new();
    for (_, id) in KIND_ICONS {
        if !valid_stickers.iter().any(|s| s == id) {
            missing.push(*id);
        }
    }
    // Fallback dupes a table entry today; keep the guard for table edits.
    if !KIND_ICONS.iter().any(|(_, id)| *id == FALLBACK_ICON)
        && !valid_stickers.iter().any(|s| s == FALLBACK_ICON)
    {
        missing.push(FALLBACK_ICON);
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
