//! Help texts per surface (split from `views`: 300-line file limit).
//! Single source with routers — parity tests pin Help≡router.
use super::super::GENERAL_HINT;

pub fn help_text() -> &'static str {
    "`/agents`   control panel: spaces, agents, ➕ spawn\n\
     `/spawn <kind> [space]`   spawn a new agent\n\
     `/space [name]`   new space + shell topic\n\
     `/model`    current model + free-Zen picker (opencode)\n\
     `/quit`     drop the agent to a shell (busy confirms)\n\
     `/kill`     close the pane completely (confirms first)\n\
     `/shell [space]`    open a fresh shell pane\n\
     `/pane [space]`     shell pane in this space, stays here\n\
     `/split`    sibling shell pane (longer side; or right|down)\n\
     `/read [n]`     recent output of focused agent (`/output [n]` too)\n\
     `/card`     re-post the question + buttons (never stuck)\n\
     `/esc`      guarded Esc: dismiss, blocked-only\n\
      `/status`   refresh agent status card\n\
      `/history [n]`  prompts you sent here (cross-device catch-up)\n\
      `/reset`    paced reset of all topics (re-sync from Herdr)\n\
      `/cancel [all|<pane>]` abort focused job(s), all = everything / exit keys-mode\n\
     `/keys <pane> y enter`   send raw keys\n\n\
     ↩️ reply to any bot message → talks to that agent\n\
     plain text → focused agent\n\n\
      alerts fire on ⛔ needs-input / ✅ finish — just reply to them"
}

/// General-topic help (extracted from the General router so parity
/// tests pin it like every other surface): topic/DM-scoped commands
/// redirect here with guidance instead of 404ing.
pub fn general_help_text() -> String {
    format!(
        "🤖 **Herdr Telegram Bot**\n\n\
     • `/agents` — open spaces & agents control panel\n\
     • `/spawn <kind> [space]` — spawn a new agent & topic\n\
      • `/shell [space]` — open a fresh shell pane & topic\n\
      • `/pane [space]` — shell pane in this space, stays here\n\
      • `/space [name]` — new space + shell topic\n\
     • `/model` — in an agent topic (or DM): model picker\n\
     • `/history [n]` — in an agent topic (or DM): recent prompts\n\
     • `/card` `/esc` — in an agent topic (or DM): fresh buttons / guarded dismiss\n\
     • `/quit` `/kill` `/split` `/read` `/output` `/status` `/keys` — inside the agent's topic (or DM)\n\
     • `/reset` — paced reset of all topics (re-sync from Herdr)\n\
      • `/cancel [all|<pane>]` — abort focused job(s), all = everything\n\n\
     {GENERAL_HINT}"
    )
}

pub fn topic_help_text(pane: &str, kind: &str) -> String {
    format!(
        "🤖 **{kind}** topic [{pane}]\n\n\
         • Plain text sends a prompt to this agent\n\
         • `/start` — show this help\n\
         • `/agents` — spaces & agents panel (here)\n\
          • `/spawn <kind> [space]` — spawn a new agent & topic\n\
         • `/read [n]` or `/output [n]` — fetch recent terminal output\n\
         • `/history [n]` — recent prompts you sent here\n\
         • `/model` — current model + free-Zen picker (opencode)\n\
         • `/quit` — drop the agent to a shell (busy confirms)\n\
         • `/kill` — close this pane completely (confirms first)\n\
         • `/card` — re-post the question + buttons (never stuck)\n\
         • `/esc` — guarded Esc: dismiss, blocked-only\n\
          • `/shell [space]` — open a fresh shell pane\n\
          • `/pane [space]` — shell pane in this space, stays here\n\
          • `/space [name]` — new space + shell topic\n\
          • `/split` — sibling shell pane (longer side; or right|down)\n\
           • `/keys y enter` — send keystrokes\n\
          • `/cancel [all|<pane>]` — abort this pane (or scope), re-prompt to resume\n\
          • `/reset` — reset this topic (re-sync from Herdr)\n\
          • `/status` — refresh agent status card\n\
          • ✏️ rename this topic = renames in herdr (kept in sync)"
    )
}

/// Help for a shell-pane topic: the pane is a terminal now. Refuses
/// (`/quit`, `/card`, `/model`, `/shell`) are documented as behavior —
/// each has a router arm answering with guidance, never silence.
pub fn shell_help_text(pane: &str) -> String {
    format!(
        "💲 shell topic [{pane}]\n\n\
         • Plain text runs as a shell command\n\
         • `opencode`, `claude`, … — run one to re-enter as agent\n\
         • `/start` — show this help\n\
         • `/agents` — spaces & agents panel (here)\n\
          • `/spawn <kind> [space]` — spawn a new agent & topic\n\
         • `/read [n]` — recent shell output (`/output [n]` too)\n\
         • `/history [n]` — recent shell commands\n\
         • `/esc` — send Esc (vim toggles mode)\n\
          • `/cancel [all|<pane>]` — abort this pane (or scope), re-prompt to resume\n\
          • `/space [name]` — new space + shell topic\n\
          • `/reset` — reset this topic (re-sync from Herdr)\n\
          • `/keys y enter` — send keystrokes\n\
          • `/kill` — close this pane completely (confirms first)\n\
         • `/pane [space]` — shell pane in this space, stays here\n\
         • `/split` — sibling shell pane (longer side; or right|down)\n\
         • `/status` — shell card\n\
         • `/quit` — already a shell (nothing to drop)\n\
         • `/card` — nothing to answer in a shell\n\
         • `/model` — no agent here (run one first)\n\
         • `/shell` — already in a shell topic (`/pane` for a second)\n\
         • ✏️ rename this topic = renames in herdr (kept in sync)"
    )
}
