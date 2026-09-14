# herdr-telegram

Your AI coding agents, in your pocket. Prompt opencode, shells, and any Herdr agent from Telegram — phone, desktop, anywhere — and get pinged the moment one needs you or finishes. Start on your PC, continue on mobile, seamlessly.

## You need

- [Herdr](https://herdr.dev) running your agents (same machine as this bot)
- A Telegram bot: [@BotFather](https://t.me/BotFather) → `/newbot` → token
- A forum supergroup for that machine: new group → enable Topics → add your bot as **admin** (it must manage topics). One group per PC keeps hosts separate.

## Setup

```sh
cargo build --release
cp .env.example .env  # fill in below
./target/release/herdr-telegram
```

| Env | What |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | token from @BotFather |
| `TELEGRAM_CHAT_ID` | your numeric id ([@userinfobot](https://t.me/userinfobot)) |
| `TELEGRAM_FORUM_CHAT_ID` | the supergroup id — one topic per agent (omit for DM-only mode) |
| `HERDR_SOCKET` | optional, default `~/.config/herdr/herdr.sock` |

## Use

- General: `/agents` `/spawn <kind> [space]` `/space [name]` `/shell [space]` `/cancel` `/help`
- Agent topic: plain text = prompt. `/read` `/model` `/quit` `/kill` `/keys` `/status` `/cancel`
- Blocked agents post answer cards — tap a button or just type the answer.
- Topics track state on their icon: 💻 working · 💬 idle · ✅ done · ❗️ blocked · 🏁 closed. Your titles are never touched.

## License

MIT
