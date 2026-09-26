pub mod emoji;
pub mod keyboards;
pub mod scope_text;
pub mod views;

pub use emoji::*;
pub use keyboards::*;
pub use views::*;

/// Single source for the fail-closed outage ack (ambiguous herdr read →
/// no write, visible retry). Dup'd literals re-drift — use this.
pub const HERDR_UNREACHABLE: &str = "⚠️ herdr unreachable — try again";

/// Single source for the unknown-target ack (explicit arg naming nothing
/// live must never answer for a different agent). 11 sites share it.
pub const UNKNOWN_TARGET: &str = "unknown target — see /agents";

/// Single source for the tap contention ack (a tap/type already owns the
/// pane — the retry is just sending again in a beat).
pub const ANSWER_IN_FLIGHT: &str = "answer already in flight — wait a beat";

/// Single source for the keys contention ack (tap/model owns pane input).
pub const TAP_MODEL_IN_FLIGHT: &str = "tap/model op in flight — wait a beat";

/// Single source for the /esc contention ack (an answer owns the pane).
pub const ESC_IN_FLIGHT: &str = "answer in flight — wait a beat, then /esc";

/// Single source for the failed-submit-blocked ack when the question card
/// is already up (or owned in flight): plain text, no second ❗ buzz.
pub const BLOCKED_SEE_CARD: &str = "⛔ blocked — see question card";

/// Single source for the masked submit-error card (`enqueue`,
/// `enqueue_blocked` empty-screen, and the dropped-card fallback share
/// it — dup'd `⚠️ error:` literals re-drift).
pub fn error_card(err: &str) -> String {
    format!("⚠️ error: {}", crate::types::mask_home(err))
}

/// Single source for the /card contention ack (an answer owns the pane).
pub const CARD_IN_FLIGHT: &str = "answer in flight — wait a beat, then /card";

/// Single source for the keys-sent ack shared by every keys arm.
pub const KEYS_SENT: &str = "⌨️ keys sent";

/// Single source for the unreadable-space ack (fail-closed: never open in
/// the wrong space).
pub const SPACE_UNREADABLE: &str = "⚠️ could not read space — try again";

/// Single source for the unknown-space ack (named space resolving to
/// nothing — fail-closed, never open in the wrong one).
pub fn unknown_space(spec: &str) -> String {
    format!("⚠️ unknown space `{spec}` — see `/agents`")
}

/// Single source for the empty-output fallback shared by every output
/// surface (pane/agent reads, shell results, topic reads).
pub const NO_OUTPUT: &str = "(no output)";

/// Single source for the typed-answer ack (every typed-answer surface).
pub fn typed_ack(pane: &str) -> String {
    format!("⌨️ typed into {pane} + ⏎")
}

/// Single source for the stranded-typewait shell refuse (typed answer
/// landing across an agent→shell flip: fail-closed, never run as shell).
pub const STALE_TYPEWAIT_SHELL: &str = "that answer was armed for a question, but this is now a shell — re-send as a shell command if intended";

/// Single source for the card-failure fallback (failed type + failed
/// repost: answer on the PC). Dup'd literals re-drift — use this.
pub const CARD_FAILED_PC: &str = "card failed too — answer on the PC";

/// Single source for the /card repost failure (blocked, but the fresh
/// card would not render: the /read hint is intentional — reuse this,
/// not CARD_FAILED_PC, which is the post-type-failure fallback).
pub const CARD_FAILED_READ_PC: &str = "⚠️ card failed — try /read, or answer on the PC";

/// Single source for the Esc keys-send failure (Esc never landed —
/// answer on the PC). Third variant beside CARD_FAILED_PC /
/// CARD_FAILED_READ_PC: dup'd literals re-drift, so use this.
pub const KEYS_FAILED_PC: &str = "⚠️ keys failed — answer on the PC";

/// Single source for the stale-arm refuse (an expired run/keys waiter
/// must never fire its write late — the retry is just tapping again).
pub const ARM_EXPIRED: &str = "that button expired — tap it again";

/// Single source for the stale-tap empty-screen ack (unverifiable tap:
/// no buttons, no pane leak — heal follows). Dup'd literals re-drift.
pub const UNKNOWN_BUTTON: &str = "unknown button — /card for fresh buttons";

/// Single source for the unknown-command ack (all surfaces).
pub const UNKNOWN_COMMAND: &str = "unknown command — `/help`";

/// Single source for the General unknown-command ack: same base with
/// the `/agents` nudge (unknown commands there are usually pane
/// commands run in the wrong topic). Distinct surface contract — not
/// a dup of [`UNKNOWN_COMMAND`].
pub const UNKNOWN_COMMAND_GENERAL: &str = "unknown command — `/help` — see /agents";

/// Single source for the General topic redirect (/card + /esc run in
/// the agent's own topic, never General). Dup'd literals re-drift.
pub const REDIRECT_TOPIC: &str =
    "open the agent's topic and run it there — each topic is one agent.";

/// Single source for the `/split` usage (`[right|down]` arms share it).
pub const USAGE_SPLIT: &str = "usage: `/split [right|down]` — bare picks the longer side";

/// Single source for the shell-topic shell refuse (`/shell` inside a
/// shell: `/pane` mints the second shell).
pub const ALREADY_SHELL_TOPIC: &str = "already in a shell topic — `/pane` for a second shell here.";

/// Single source for the shell `/card` refuse (shells hold no dialogs).
pub const SHELL_NO_CARD: &str = "this is a shell — nothing to answer.";

/// Single source for the General bare-text hint (router + help share it).
pub const GENERAL_HINT: &str =
    "💡 To talk to an agent, please open its dedicated topic or use `/agents` to spawn one.";

/// Single source for the orphan-topic refuse (a threaded message whose
/// thread maps to nothing: reset/remint orphan or user-made topic).
/// Posted in-thread (dead threads fail silently on send, live ones get
/// guidance) instead of routing to General — General control (`/reset`,
/// `/spawn`, …) must never fire from a corpse thread, and bare prompts
/// must never route with the wrong id for follow-ups.
pub const UNKNOWN_TOPIC: &str =
    "this topic is no longer linked to an agent — see General or `/agents`.";

/// Single source for the topic cancel ack (agent + shell flavors share it).
pub fn cancel_ack(pane: &str, cancelled: bool) -> String {
    if cancelled {
        format!("✋ cancelled {pane}")
    } else {
        format!("nothing running for {pane}")
    }
}

/// Single source for the shell `/quit` refuse (already a shell).
pub const ALREADY_SHELL: &str = "already in shell — type any command.";

/// Single source for the shell `/model` refuse (no agent here).
pub const NO_AGENT_RUN_ONE: &str = "no agent here — run one (`opencode`, `claude`, …) to start it.";

/// Single source for the quit keys-send failure (quit never landed).
pub const QUIT_KEYS_FAILED_PC: &str = "⚠️ quit keys failed — quit on the PC";

/// Single source for the not-blocked refuse (`/card` + `/esc` share it).
pub fn not_blocked_ack(status: &str, keys_hint: Option<&str>) -> String {
    match keys_hint {
        Some(hint) => format!(
            "not blocked (status={status}) — Esc would hit live work; use {hint} if you really mean it"
        ),
        None => format!("not blocked (status={status}) — nothing to answer"),
    }
}

/// Single source for the reset contention ack (paced + single share
/// it — dup'd literals re-drift).
pub const RESET_BUSY: &str = "⚠️ reset already in progress, try again shortly";

/// Single source for the reset permission abort (paced + single share it).
pub const RESET_NO_PERM: &str = "⚠️ reset aborted: bot lacks 'can_manage_topics' admin permission";

/// Single source for the forum-only reset refuse (paced + single share
/// it — dup'd literals re-drift).
pub const RESET_FORUM_ONLY: &str = "⚠️ Reset is only available in forum supergroup mode";

/// Single source for the agent-gone shell notice (callback `a:` arm +
/// DM `/status` share it — dup'd literals re-drift).
pub fn shell_gone_text(pane: &str) -> String {
    format!("{pane} is now a shell pane (agent gone)")
}

/// Single source for the spawn-failure ack (`/spawn` + button-spawn
/// share it — dup'd literals re-drift).
pub fn spawn_failed(e: &str) -> String {
    format!("⚠️ spawn failed: {}", crate::types::mask_home(e))
}
