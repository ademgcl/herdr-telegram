# AGENTS.md

> Keep this file minimal — fewest words that still guide. No prose, no duplication.

herdr-telegram: Telegram (DMs + forum topics) ↔ Herdr multiplexer over local Unix socket RPC.

---

## 1. Core Rule: 300-Line File Limit

> [!IMPORTANT]
> No source file may exceed **300 lines**. Near the limit, split into single-responsibility submodules.

Verify: `wc -l $(find src -name '*.rs') AGENTS.md` before committing.

---

## 2. Architecture

```
src/
├── main.rs                 # Single-instance guard, init, main loop
├── types.rs                # Core types, AgentRow, error alias
├── config.rs               # Env loading
├── state/                  # Shared State
│   ├── mod.rs              # State struct, focus/offset files
│   └── jobs.rs             # Job retire, pane cleanup
├── herdr/                  # Herdr layer (owns socket I/O)
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Facade re-export
│   ├── rpc.rs              # Unix socket transport
│   ├── agents.rs           # Agent list/get/read/keys
│   ├── panes.rs            # Pane list/read/input/keys/tabs/splits
│   ├── workspaces.rs       # Workspace ensure + spawn
│   ├── screens.rs          # Screen snapshots
│   ├── labels.rs           # Pane labels
│   └── events.rs           # Event stream + reconnect
├── telegram/               # Telegram layer (owns HTTP I/O)
│   ├── mod.rs              # Re-exports
│   ├── client.rs           # Transport + setup
│   ├── messages.rs         # Send/edit/typing/callback/unpin
│   ├── forum.rs            # Forum-topic API
│   ├── errors.rs           # Missing/not-modified predicates
│   ├── polling.rs          # Long-poll transport
│   └── router.rs           # Update routing
├── topics/                 # Forum topic management
│   ├── mod.rs              # Re-exports
│   ├── storage/            # Pane↔thread map (mod.rs + tests.rs)
│   ├── names.rs            # Tags, titles, context icons
│   ├── manager/            # Lifecycle (mod.rs + lifecycle.rs + titles.rs)
├── handlers/               # Update handlers
│   ├── mod.rs              # Re-exports
│   ├── dm.rs               # DM dispatcher
│   ├── dm_info.rs          # DM /agents /keys /read /status
│   ├── dm_lifecycle.rs     # DM /quit /kill /shell /space /spawn
│   ├── dm_model.rs         # DM /model
│   ├── dm_prompt.rs        # DM typewait + bare-text routing
│   ├── escape.rs           # /card + /esc (topic/DM/shell)
│   ├── forum.rs            # Topic messages + commands
│   ├── general.rs          # General-topic commands
│   ├── callback.rs         # Callback dispatcher
│   ├── callback_parse.rs   # Routing cuts + stale-pane guards
│   ├── callback_spawn.rs   # Workspace/agent spawn
│   ├── callback_model.rs   # Model taps
│   ├── dialog/             # Blocked cards (mod.rs + tests.rs)
│   ├── tap.rs              # Facade re-export
│   ├── tap_classify.rs     # Post-tap classification (pure)
│   ├── tap_keys.rs         # Key-send + re-read
│   ├── tap_answer.rs       # Button-tap entry
│   ├── tap_input.rs        # Typed answers + waiters
│   ├── interactive.rs      # Static key maps
│   ├── kill.rs             # Kill confirm flow
│   ├── model.rs            # Model picker
│   ├── model_parse.rs      # Picker list parsing
│   ├── model_scan.rs       # Column-split helpers
│   ├── reset.rs            # Paced topic reset (read-only Herdr)
│   ├── shell.rs            # Facade re-export
│   ├── shell_common.rs     # Reply format, settle wait
│   ├── shell_run.rs        # Shell command run
│   ├── shell_lifecycle.rs  # Quit-to-shell
│   ├── shell_provision.rs  # Shell open/split
│   ├── shell_topic.rs      # Shell-topic commands
│   ├── space.rs            # Workspace creation
│   ├── target.rs           # DM pane-target resolution
│   └── titles.rs           # Pane↔topic title sync
├── jobs/                   # Prompt execution
│   ├── mod.rs              # Re-exports
│   ├── job.rs              # Per-pane job state
│   ├── echo.rs             # Prompt-echo recognition
│   ├── episode.rs          # Stall-episode gate
│   ├── runner.rs           # Prompt watcher
│   ├── enqueue.rs          # Prompt submit
│   ├── stall.rs            # Mid-run stall watch
│   ├── finalize.rs         # Final card post + retry
│   ├── arbitrate.rs        # Stream vs settled arbitration
│   ├── notices/            # Limit alerts (mod + types + patterns + detect + card)
│   ├── persist.rs          # Durable prompt intent
│   ├── recover.rs          # Boot re-arm of watchers
│   ├── segment.rs          # Fresh-reply extraction
│   ├── stream.rs           # Event stream client
│   └── filter.rs           # TUI chrome filtering
├── notifier/               # Status & alerts
│   ├── mod.rs              # Re-exports
│   ├── status.rs           # Transition observer, flap dampener
│   ├── cards.rs            # Debounced spontaneous pushes
│   └── reconcile.rs        # Watchdog + limit scan
└── ui/                     # Presentation
    ├── mod.rs              # Re-exports
    ├── emoji.rs            # Status emoji
    ├── keyboards.rs        # Inline keyboards
    └── views.rs            # Text views + sizing
```

---

## 3. Forum Topics (`TELEGRAM_FORUM_CHAT_ID` set)

- One topic per pane. Titles sync 1:1 with pane names, Format B (`[space] label · agent`).
- Renames converge both ways via stored-title compare (≤60s watchdog); down-window: last-writer-wins.
- Icon = context only (💻 agent · 💬 shell, set once, user customs kept). Status in cards/typing (+1 identity pin).
- Buzz: answers, `blocked`, limit stalls. `done`/`idle` only on fresh-output debounce.
- Blocked cards follow content; taps edit in place; typed answers atomic + verified.
- Topic: text = prompt; `/read` `/output` `/keys` `/status` `/model` `/quit` `/kill` `/shell` `/space` `/split` `/card` `/esc` `/cancel` `/help` (`/space` global; `/shell` agent-topics only).
- General: `/agents` `/spawn` `/space` `/shell` `/model` `/start` `/reset` `/cancel` `/help`.
- Down-window spontaneous completions stay silent (no baseline).

---

## 4. DM Mode

- Reply to target; `/agents` navigator; text → focused (or sole) agent.

---

## 5. Guidelines

- cargo; tokio multi-thread; `Res<T>`; no async mutex across sleep/RPC; herdr/telegram own I/O.
- No personal identifiers in tracked files/commits; secrets in untracked `.env` only.

---

## 6. Reply Style

- CTO summary: minimal, bulleted, every idea kept, nothing extra.
