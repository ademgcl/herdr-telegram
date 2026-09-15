# AGENTS.md

herdr-telegram: Telegram (DMs + supergroup forum topics) ↔ Herdr terminal/agent multiplexer via local Unix socket RPC.

---

## 1. Core Rule: 300-Line File Limit

> [!IMPORTANT]
> No source file may exceed **300 lines**. Near the limit, split into single-responsibility submodules.

Verify: `wc -l $(find src -name '*.rs') AGENTS.md` before committing.

---

## 2. Architecture

```
src/
├── main.rs                 # Single-instance guard, init, main select loop
├── types.rs                # Core types, error alias, AgentRow, AgentDetail
├── config.rs               # Env loading & Cfg
├── state.rs                # Thread-safe shared State
├── herdr/                  # Herdr communication layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Unix socket RPC
│   ├── labels.rs           # Pane labels (rename + list parse)
│   └── events.rs           # Event stream + auto-reconnect
├── telegram/               # Telegram API layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Bot API HTTP client
│   ├── polling.rs          # Long-poll, offsets, stale filter
│   └── router.rs           # Update routing
├── topics/                 # Forum topic management
│   ├── mod.rs              # Re-exports
│   ├── storage.rs          # Pane↔thread map in topics.state
│   ├── names.rs            # Tags, 1:1 title rule, icons, done→idle decay
│   └── manager.rs          # Creation (pane-id named), icon + title sync, lifecycle
├── handlers/               # Update handlers
│   ├── mod.rs              # Re-exports
│   ├── dm.rs               # DM messages + commands
│   ├── forum.rs            # Topic messages + commands
│   ├── callback.rs         # Inline keyboard callbacks
│   ├── dialog.rs           # Blocked cards, content-addressed refresh
│   ├── tap.rs              # Taps (in-place update) + typed answers
│   ├── interactive.rs      # Static key maps
│   ├── kill.rs             # Kill confirm flow
│   ├── model.rs            # Opencode model picker
│   ├── model_parse.rs      # Picker list parsing
│   ├── model_scan.rs       # Column-split helpers
│   └── shell.rs            # Agentless shell panes
│   └── titles.rs           # 1:1 pane↔topic title sync (both directions)
├── jobs/                   # Agent prompt execution
│   ├── mod.rs              # Re-exports
│   ├── job.rs              # Per-pane job state, cancel notify
│   ├── runner.rs           # Prompt submit + watcher (live → final card)
│   ├── finalize.rs         # Settle arbitration: stream vs settled screen
│   ├── notices.rs          # Limit/quota stall alerts
│   ├── segment.rs          # Fresh-reply extraction
│   ├── stream.rs           # Event stream client, delta helpers
│   └── filter.rs           # TUI chrome filtering
├── notifier/               # Status & alert dispatching
│   ├── mod.rs              # Re-exports
│   ├── status.rs           # Transition observer, flap dampener, alerts
│   ├── cards.rs            # Debounced spontaneous pushes
│   └── reconcile.rs        # Watchdog + background limit scan
└── ui/                     # Presentation & formatters
    ├── mod.rs              # Re-exports
    ├── emoji.rs            # Status emoji
    ├── keyboards.rs        # Inline keyboards
    └── views.rs            # Text views + message sizing
```

---

## 3. Forum Topics (`TELEGRAM_FORUM_CHAT_ID` set)

- One topic per pane (agents and shells alike).
- Titles sync 1:1 with herdr pane names: pane label when set, else the unique pane id (`w8:p1`). Last synced title persists in `topics.state`.
- Either side renames: native topic rename → `pane.rename`; herdr-side rename → topic renamed on the ≤60s watchdog. Stored-title compare both ways, so edits converge without loops.
- State lives on the **icon**, synced silently (never notifies): 💻 working · 💬 idle · ✅ done · ❗️ blocked · 🏁 closed · ❓ unknown. Fresh `done` decays to idle after 15 quiet min.
- Buzz: answers, `blocked`, usage-limit stalls. `done`/`idle` post only if a debounce holds with fresh output; empty settles stay silent.
- Blocked cards follow content (dialogs turn over with no status change): repeats silent, new dialog posts/updates. Taps edit the card in place (buttons stripped on resume); typed answers use atomic `pane.send_input`, verified on-screen.
- In-topic plain text = prompt; commands in context: `/read` `/keys` `/status` `/model` `/quit` `/kill` `/shell` `/space` `/cancel`.
- General topic: `/agents` `/spawn` `/space` `/shell` `/help`.

---

## 4. DM Mode

- Reply to any card to target that agent; `/agents` navigator; plain text → focused (or sole) agent.

---

## 5. Guidelines

- cargo; tokio multi-thread; `Res<T>` errors; no async mutexes across sleep/long RPCs; no socket/HTTP logic in handlers.

---

## 6. Reply Style

- CTO summary: minimal, bulleted, every idea kept, nothing extra.
