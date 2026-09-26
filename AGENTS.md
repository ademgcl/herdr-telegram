# AGENTS.md

> Fewest words that still guide. No prose, no duplication. Challenge every line.

herdr-telegram: Telegram (DMs + forum topics) ↔ Herdr multiplexer over local Unix socket RPC.

## 1. Rules

- No source file over **300 lines** — split near the limit (tests to sibling `*_tests.rs` or `tests.rs`).
- Verify: `cargo test`, clippy clean, `wc -l $(find src -name '*.rs') AGENTS.md` before committing.
- cargo; tokio multi-thread; `Res<T>`; no async mutex across sleep/RPC; I/O via herdr/telegram clients; jobs owns EvStream.
- Min code, max greatness: smallest diff that fully fixes, zero dead code; A+ or rework.
- Fail-closed: ambiguous reads/errors → no write, no buzz.
- No personal identifiers in tracked files/commits; secrets in untracked files only.
- New command needs: router arm + help + menu scope + README + test, or not at all. Help ≡ router (parity test per surface; only `/start`+`/help` exempt).
- State grows by split: new map needs expiry + prune.
- Cards: strip before slow RPC, reconcile every arm; overwrite-only, text untouched; contention silent (no strip).
- Loop notices get burst guards: time-bounded per-key dedup (pure helper + test), never forever-mute.
- Single source per string: dup'd literals re-drift.

## 2. Layout

- `main.rs` init/loop · `types.rs` · `config.rs` · `ctl.rs` + `ctl_auth.rs` + `ctl_cmd.rs` local control socket · `state/` shared State · `herdr/` socket layer · `telegram/` HTTP layer · `topics/` forum topics · `handlers/` updates · `jobs/` prompt watchers · `notifier/` alerts · `ui/` views
- Details live in per-file module docs — read them, don't duplicate here.

## 3. Behavior

- One topic per pane; titles 1:1 with herdr TAB names (split tabs: pane label) (`[space] label`, kind in icon); herdr→tg ≤60s watchdog, tg→herdr immediate adopt, stale replays dropped, no chat echo.
- Icon = live kind (glyphs + fallback, customs kept); status in cards/typing (never pin). Buzz: answers, `blocked`, stuck limits.
- Blocked cards follow content; taps edit in place; `/card` reposts, `/esc` dismisses (blocked-only; shell: Esc raw, card refused).
- Topic: text = prompt + commands; General: panel + scoped cmds (see `/help`); DM: full set, reply→focus→sole-agent.
- Prompts queue FIFO per pane while a turn is owed (one final each; RAM-only, `/cancel` drops); photos attach as `[attached image: path]` (caption = prompt), shells/General refuse visibly.
- Spontaneous settles stay silent when post-prompt, empty, or moved-on.

## 4. Replies

- CTO summary: minimal, bulleted, every idea kept, nothing extra.
