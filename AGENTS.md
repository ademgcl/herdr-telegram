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
├── state/                  # Thread-safe shared State
│   ├── mod.rs              # State struct, files (focus/offset), routing
│   └── jobs.rs             # Job retire, intent durability, pane cleanup
├── herdr/                  # Herdr communication layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Facade re-export (paths stable)
│   ├── rpc.rs              # Unix socket transport (rpc/rpc_t/ping)
│   ├── agents.rs           # Agent list/get/read/keys
│   ├── panes.rs            # Pane list/read/input/keys/tabs/splits
│   ├── workspaces.rs       # Workspace ensure + agent spawn
│   ├── screens.rs          # Screen snapshots (tail/visible/adaptive)
│   ├── labels.rs           # Pane labels (rename + list parse)
│   └── events.rs           # Event stream + auto-reconnect
├── telegram/               # Telegram API layer
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Core transport + setup (call/new/redact)
│   ├── messages.rs         # Message UX (send/edit/typing/callback/unpin)
│   ├── forum.rs            # Forum-topic API (create/close/title/icon)
│   ├── errors.rs           # Error predicates (missing/not-modified)
│   ├── polling.rs          # Long-poll transport (offsets in main/state, stale filter in router)
│   └── router.rs           # Update routing
├── topics/                 # Forum topic management
│   ├── mod.rs              # Re-exports
│   ├── storage.rs          # Pane↔thread map in topics.state
│   ├── names.rs            # Tags, 1:1 title rule, icons, done→idle decay
│   └── manager.rs          # Creation (friendly tag·space named), icon + title sync, lifecycle
├── handlers/               # Update handlers
│   ├── mod.rs              # Re-exports
│   ├── dm.rs               # DM dispatcher (waiters → commands → prompt)
│   ├── dm_info.rs          # DM /agents /keys /read /status
│   ├── dm_lifecycle.rs     # DM /quit /kill /shell /space /spawn
│   ├── dm_model.rs         # DM /model
│   ├── dm_prompt.rs        # DM typewait + bare-text routing
│   ├── forum.rs            # Topic messages + commands
│   ├── general.rs          # General-topic commands
│   ├── callback.rs         # Callback dispatcher
│   ├── callback_parse.rs   # Routing cuts + stale-pane guards
│   ├── callback_spawn.rs   # Callback workspace/agent spawn
│   ├── callback_model.rs   # Callback model taps
│   ├── dialog.rs           # Blocked cards, content-addressed refresh
│   ├── tap.rs              # Facade re-export (paths stable)
│   ├── tap_classify.rs     # Post-tap classification (pure)
│   ├── tap_keys.rs         # Key-send + re-read
│   ├── tap_answer.rs       # Button-tap entry (in-place card update)
│   ├── tap_input.rs        # Typed answers + run/key waiters
│   ├── interactive.rs      # Static key maps
│   ├── kill.rs             # Kill confirm flow
│   ├── model.rs            # Opencode model picker
│   ├── model_parse.rs      # Picker list parsing
│   ├── model_scan.rs       # Column-split helpers
│   ├── shell.rs            # Facade re-export (paths stable)
│   ├── shell_common.rs     # Reply format, settle wait, card text
│   ├── shell_run.rs        # Shell command run
│   ├── shell_lifecycle.rs  # Quit-to-shell
│   ├── shell_provision.rs  # Shell open/split
│   ├── shell_topic.rs      # Shell-topic command routing
│   ├── space.rs            # Workspace creation (/space)
│   ├── target.rs           # DM pane-target resolution
│   └── titles.rs           # 1:1 pane↔topic title sync (both directions)
├── jobs/                   # Agent prompt execution
│   ├── mod.rs              # Re-exports
│   ├── job.rs              # Per-pane job state, cancel notify
│   ├── episode.rs          # Stall-episode gate for mid-run alerts
│   ├── runner.rs           # Prompt watcher (live → final card)
│   ├── enqueue.rs          # Prompt submit (deliver-first, last-wins books)
│   ├── stall.rs            # Mid-run limit/stall watch (per-episode buzz)
│   ├── finalize.rs         # Final card post + delivery retry
│   ├── arbitrate.rs        # Settle arbitration: stream vs settled screen
│   ├── notices/            # Limit/quota stall alerts
│   │   ├── mod.rs          # Re-exports
│   │   ├── types.rs        # LimitHit, kinds, stuck gate
│   │   ├── patterns.rs     # Match tables (strong/weak/context)
│   │   ├── detect.rs       # Screen scanning (priority-ordered)
│   │   └── card.rs         # Buzzing card rendering
│   ├── persist.rs          # Durable prompt intent (jobs.state)
│   ├── recover.rs          # Boot re-arm of orphaned watchers
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
- Titles sync 1:1 with herdr pane names: pane label when set, else the friendly default (`o2 · tg`). Last synced title persists in `topics.state`.
- Either side renames: native topic rename → `pane.rename`; herdr-side rename → topic renamed on the ≤60s watchdog. Stored-title compare both ways, so edits converge without loops.
- Known limit: a rename made while the bot is down can diverge (no read API for topic names) — last-observed-writer wins; redelivered updates heal it.
- State lives on the **icon**, synced silently (never notifies): 💻 working · 💬 idle · ✅ done · ❗️ blocked · 🏁 closed · ❓ unknown (custom-emoji IDs via `icon_emoji_id`; card glyphs differ, see `ui/emoji.rs`). Fresh `done` decays to idle after 15 quiet min.
- Buzz: answers, `blocked`, usage-limit stalls. `done`/`idle` post only if a debounce holds with fresh output; empty settles stay silent.
- Blocked cards follow content (dialogs turn over with no status change): repeats silent, new dialog posts/updates. Taps edit the card in place (buttons stripped on resume); typed answers use atomic `pane.send_input`, verified on-screen.
- In-topic plain text = prompt; commands in context: `/read` `/output` `/keys` `/status` `/model` `/quit` `/kill` `/shell` `/space` `/split` `/cancel` `/help`.
- General topic: `/agents` `/spawn` `/space` `/shell` `/model` (redirect) `/start` `/cancel` `/help`.
- Known limit: spontaneous (non-prompt) completions that finish while the bot is down stay silent (no baseline to diff) — last-observed-writer-wins otherwise.

---

## 4. DM Mode

- Reply to any card to target that agent; `/agents` navigator; plain text → focused (or sole) agent.

---

## 5. Guidelines

- cargo; tokio multi-thread; `Res<T>` errors; no async mutexes across sleep/long RPCs; no raw socket/HTTP in handlers — herdr/telegram layers own I/O.

---

## 6. Reply Style

- CTO summary: minimal, bulleted, every idea kept, nothing extra.
