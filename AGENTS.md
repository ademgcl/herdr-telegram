# AGENTS.md

> Fewest words that still guide. No prose, no duplication. Challenge every line.

herdr-telegram: Telegram (DMs + forum topics) ↔ Herdr multiplexer over local Unix socket RPC.

## 1. Rules

- No source file over **300 lines** — split near the limit.
- Verify: `wc -l $(find src -name '*.rs') AGENTS.md` before committing.
- cargo; tokio multi-thread; `Res<T>`; no async mutex across sleep/RPC; herdr/telegram own I/O.
- Min code, max greatness: smallest diff that fully fixes; A+ or rework.
- No personal identifiers in tracked files/commits; secrets in untracked `.env` only.

## 2. Layout

- `main.rs` init/loop · `types.rs` · `config.rs` · `ctl.rs` + `ctl_auth.rs` + `ctl_cmd.rs` local control socket · `state/` shared State · `herdr/` socket layer · `telegram/` HTTP layer · `topics/` forum topics · `handlers/` updates · `jobs/` prompts · `notifier/` alerts · `ui/` views
- Details live in per-file module docs — read them, don't duplicate here.

## 3. Behavior

- One topic per pane; titles 1:1 with pane names (`[space] label`, kind in icon); renames converge both ways (≤60s watchdog, else last-writer-wins).
- Icon = live kind (per-agent glyph, user customs kept); status in cards/typing (never pin). Buzz: answers, `blocked`, limit stalls.
- Blocked cards follow content; taps edit in place; `/card` reposts, `/esc` dismisses (blocked-only).
- Topic: text = prompt + commands; General: `/agents` `/spawn` `/space` `/shell` `/model` `/start` `/reset` `/cancel` `/help`; DM: reply to target.
- Down-window spontaneous completions stay silent.

## 4. Replies

- CTO summary: minimal, bulleted, every idea kept, nothing extra.
