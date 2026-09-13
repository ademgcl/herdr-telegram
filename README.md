# herdr-telegram

Telegram ↔ Herdr bridge. DM + forum-topic control for Herdr agents via Unix socket RPC.

## Setup
```sh
cargo build --release
```

Env:
```
TELEGRAM_BOT_TOKEN=...
TELEGRAM_CHAT_ID=<owner_id>
TELEGRAM_FORUM_CHAT_ID=<supergroup_id> # optional, enables 1-topic-per-agent
HERDR_SOCKET=~/.config/herdr/herdr.sock # optional
```

Run: `cargo run`

## Use
- General: `/agents` `/spawn <kind> [space]` `/newspace <name>` `/help` `/cancel`
- In agent topic: plain text = prompt, plus `/read` `/keys` `/status` `/cancel`
- In DM: reply to card, or `/agents` to focus, then plain text = prompt.
