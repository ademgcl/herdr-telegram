//! Stable per-pane tags (`2`): kept as persisted ids AND shown in the
//! title (`2 . tg`) — see [`format_title`]. They used to carry the
//! agent's short code (`o2`) so two kinds in one space could not collide;
//! the kind now lives in the topic ICON, so the number alone is both the
//! identity and what you see. Live status surfaces in cards and the
//! typing indicator (plus one unpinned identity card per topic).

mod chrome;
mod core;
mod format;
mod suffix;

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

/// Herdr's own pane number: `wZ:p7` → `7`.
///
/// Herdr numbers panes per WORKSPACE, monotonically and never reusing a
/// freed number, and the counter is shared across a space's tabs — a new
/// tab's first pane continues the sequence rather than restarting. So the
/// number is unique within the space, stable for the pane's life, and
/// identical to what herdr's UI shows. Splits need no special case: they
/// take the next number in the space.
///
/// `None` for a pane id with no numeric tail, so callers fall back
/// rather than invent a number herdr never had.
pub fn pane_number(pane: &str) -> Option<u32> {
    // `wZ:p7` → `p7` → `7`. The `p` is herdr's pane marker, not part of
    // the number; a bare `7` is accepted too.
    let tail = pane.rsplit_once(':')?.1.trim();
    digits_of(tail.strip_prefix('p').unwrap_or(tail))
}

/// Trailing digits of a tag, as a number: `2` from `2`, `o2`, or `p2`.
fn digits_of(tag: &str) -> Option<u32> {
    let digits: String = tag
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Smallest positive `n` unused in this space. Deterministic for a given
/// tag set; gaps from closed agents are refilled. `kind` is no longer part
/// of the id — the icon carries it — but kept in the signature so callers
/// (and their call sites) do not churn.
/// The pane's herdr number when it has one, so the topic title and
/// herdr's UI show the same digit. Uniqueness is free: herdr never
/// reuses a number inside a workspace, and a space is a workspace.
///
/// Falls back to the smallest unused `n` for a pane id with no numeric
/// tail (never seen from herdr; keeps storage total).
pub fn assign(existing: &[String], _kind: &str, pane: Option<&str>) -> String {
    if let Some(n) = pane.and_then(pane_number) {
        return n.to_string();
    }
    let used: std::collections::HashSet<u32> =
        existing.iter().filter_map(|t| digits_of(t)).collect();
    let mut n = 1u32;
    while used.contains(&n) {
        n += 1;
    }
    n.to_string()
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
#[path = "mod_tests.rs"]
mod tests;
