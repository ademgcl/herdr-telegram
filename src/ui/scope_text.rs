//! Cross-surface command scope: shared redirect/usage texts, count
//! parsers and the `/keys` target guard. Single source for every router
//! arm + help text that names another surface's scope (dup'd literals
//! re-drift). Split from `views` (300-line file limit).
//!
//! Surface architecture (do not re-litigate per command): topics are
//! own-pane-only (no target grammar in either topic router), DM is the
//! addressing surface (resolve_target/dm_pane). Identical spelling may
//! therefore differ DM↔topic by design — usage errors, never silent
//! wrong-pane behavior.

/// Default `/read` window per flavor (intentionally different: agent
/// scrollback runs deeper than shell viewports — do not unify).
pub const TOPIC_READ_DEFAULT: u32 = 80;
pub const SHELL_READ_DEFAULT: u32 = 60;
/// Line-count cap shared by every `/read` surface (mirrors DM clamp).
pub const READ_CAP: u32 = 400;

/// Redirect for topic/DM commands typed in General (one arm covers all
/// seven — same shape as the existing `/model` redirect).
pub const REDIRECT_TOPIC_SCOPED: &str =
    "that command runs in its topic (or DM) — open the agent's topic and run it there.";

/// Own-pane-only usage errors (fail-closed over silent-wrong): a target
/// that names another pane is refused, never served from this one.
pub const USAGE_READ_TOPIC: &str =
    "usage: `/read [n]` — recent output of this topic (this pane only; DM reads any pane).";
pub const USAGE_HISTORY_TOPIC: &str =
    "usage: `/history [n]` — recent prompts here (this pane only).";
pub const USAGE_HISTORY_DM: &str =
    "usage: `/history [n]` — recent prompts (counts only; open a pane's topic for its history).";
pub const USAGE_KEYS_TOPIC: &str =
    "usage: `/keys <key> [key...]` sends to this topic's pane — DM `/keys <pane>` targets others.";
pub const USAGE_RESET_TOPIC: &str =
    "usage: bare `/reset` resets this topic — General/DM `/reset <pane>` targets others.";

/// Bare `/keys` usage (own pane, both flavors) — the target guard
/// (`USAGE_KEYS_TOPIC`) covers pane-shaped first tokens.
pub const USAGE_KEYS_BARE: &str = "usage: `/keys y enter`";

/// Herdr unreadable mid-command: refuse with retry (never fall through
/// to a send on an unverified target).
pub const HERDR_RETRY: &str = "⚠️ herdr status unreadable — try again";

/// Strict line-count parse: bare → default, single integer → clamped,
/// anything else → None (caller posts usage). i64-wide like DM so
/// huge/negative clamp instead of erroring; extra tokens never silently
/// drop (that was the wrong-pane hole). Pure for tests.
pub fn parse_count(arg: &str, default: u32, cap: u32) -> Option<u32> {
    let t = arg.trim();
    if t.is_empty() {
        return Some(default);
    }
    if t.split_whitespace().count() != 1 {
        return None;
    }
    t.parse::<i64>()
        .ok()
        .map(|n| n.clamp(1, cap as i64) as u32)
}

/// True when a topic `/keys` first token names a foreign target and must
/// refuse instead of becoming keystrokes: an exact pane id, any agent
/// kind (ambiguity still refuses — kinds must never be typed blind), or
/// pane-shaped text (`:` covers shell/dead panes no row lists). Pure
/// over injected lists for tests; callers fetch one `list_agents` and
/// refuse-retry when it fails. Ordinary first words (`y`, `enter`)
/// return false — own-pane keys still send.
pub fn keys_first_blocked(first: &str, panes: &[String], kinds: &[String]) -> bool {
    panes.iter().any(|p| p == first)
        || kinds.iter().any(|k| k == first)
        || first.contains(':')
}

#[cfg(test)]
#[path = "scope_text_tests.rs"]
mod tests;
