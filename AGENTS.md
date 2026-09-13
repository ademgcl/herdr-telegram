# AGENTS.md

Welcome to **herdr-telegram**. This document defines the engineering standards, architecture, and agent rules for developing and maintaining this codebase.

---

## 1. Core Rule: File Size Limit

> [!IMPORTANT]
> **Strict Rule**: No single source code file in this repository may exceed **300 lines of code**.
> If a file approaches 300 lines, it must be split into logical submodules or separate files with single responsibilities.

### Verification
Run `wc -l $(find src -name '*.rs') AGENTS.md` before committing to ensure every file satisfies this rule.

---

## 2. Project Architecture

The service bridges **Telegram** (Direct Messages and Supergroup Forum Topics) with the **Herdr** terminal/agent multiplexer via local Unix domain socket RPC.

```
src/
├── main.rs                 # Single-instance guard, service initialization, main select loop
├── types.rs                # Core types, error alias, AgentRow, WorkspaceRow
├── config.rs               # Environment variable loading & Cfg model
├── state.rs                # Central thread-safe State & synchronization primitives
├── herdr/                  # Herdr communication layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Async Unix socket RPC (agent.*, workspace.*, pane.*, tab.*)
│   └── events.rs           # Herdr event stream subscriber and auto-reconnect
├── telegram/               # Telegram API layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Telegram Bot API client (HTTP requests, send, edit, topics)
│   ├── polling.rs          # Long-polling loop, offset management & stale update filter
│   └── router.rs           # Update routing (DMs, Forum Topics, Callbacks)
├── topics/                 # Forum topic management
│   ├── mod.rs              # Re-exports
│   ├── storage.rs          # Pane <-> thread mapping persistence (~/.local/share/herdr-telegram/topics.json)
│   └── manager.rs          # Automatic topic creation, title updates, and lifecycle tracking
├── handlers/               # Update handlers
│   ├── mod.rs              # Re-exports
│   ├── dm.rs               # Private 1-on-1 message & command handling
│   ├── forum.rs            # Supergroup forum topic message & command handling
│   └── callback.rs         # Inline keyboard callback queries (spaces, agents, spawn, keys, run)
├── jobs/                   # Agent prompt execution
│   ├── mod.rs              # Re-exports
│   ├── job.rs              # Prompt job state, cancellation notify, FIFO queue
│   └── runner.rs           # Prompt submission, timeout resilience, multi-round watch loop
├── notifier/               # Status & alert dispatching
│   ├── mod.rs              # Re-exports
│   ├── status.rs           # Status transition observer, flap dampener, alert delivery
│   └── reconcile.rs        # Watchdog reconciliation across all live agents and topics
└── ui/                     # Presentation & formatters
    ├── mod.rs              # Re-exports
    ├── emoji.rs            # Status emoji mappings
    ├── keyboards.rs        # Inline keyboard builders
    └── views.rs            # Text views for spaces, agent cards, and message sizing
```

---

## 3. Telegram Forum Topics Integration

When `TELEGRAM_FORUM_CHAT_ID` is set:
1. **One Topic Per Agent**: Every active Herdr agent has a dedicated Telegram forum topic.
2. **Topic Titles**: `{emoji} {tag} · {space}` — short kind code + stable
   per-kind counter + workspace label (usually the project folder, capped
   at 20 chars), e.g. `🔄 o2 · herdr-telegram`. The badge tracks status
   (green idle · yellow working · blue done · rose blocked): `working` and
   `blocked` rename immediately, settles follow once debounce confirms them
   (~15s) — fast yet flicker-free. Renames never notify. Tags persist in
   `topics.state`, so restarts never reshuffle names.
3. **Push Discipline**: Only answers and `blocked` (needs-input) buzz.
   `done`/`idle` settles wait out a short debounce and post only if still
   settled with fresh output; empty settles stay silent.
3. **Direct In-Topic Prompts**: Any message posted inside an agent's topic is dispatched directly to that agent as a prompt.
4. **Contextual Commands**: `/read`, `/keys`, `/cancel`, and `/status` run in the context of the topic's agent.
5. **General Topic**: Serves as the dashboard with `/agents`, `/spawn`, `/spaces`, `/newspace`, and `/help`.

---

## 4. Direct Messages (DM Mode)

Authorized owners can also control agents directly in private chats:
- Reply to any alert/card to target that agent.
- Use `/agents` for the interactive navigator.
- Plain text routes to the focused agent or the sole active agent.

---

## 5. Coding & Contribution Guidelines

- **Package Management**: Use `cargo` for Rust builds and dependencies.
- **Async Runtime**: Use `tokio` multi-threaded runtime.
- **Error Handling**: Use `Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>`.
- **Concurrency**: Avoid holding async mutexes across unbounded sleep / long RPCs.
- **Clean Separation**: Handlers should not contain low-level socket or HTTP serialization logic.
