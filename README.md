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

## Run

```sh
cp dev.herdr.telegram.plist ~/Library/LaunchAgents/ && launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.herdr.telegram.plist  # auto-restart on crash/reboot, logs to bot.log
launchctl bootout gui/$(id -u)/dev.herdr.telegram  # stop service before manual runs, or the single-instance guard + Telegram getUpdates Conflict will fight it
launchctl bootout gui/$(id -u)/dev.herdr.telegram; rm ~/Library/LaunchAgents/dev.herdr.telegram.plist  # uninstall
```

## Use

- General: `/agents` `/spawn <kind> [space]` `/space [name]` `/shell [space]` `/cancel` `/help`
- Agent topic: plain text = prompt. `/read` `/output` `/model` `/quit` `/kill` `/keys` `/status` `/shell` `/space` `/split` `/cancel` `/help`
- Spaces, agents, shells — the whole Herdr tree is drivable from chat: `/space [name]` makes a workspace and drops you in its shell topic.
- Blocked agents post answer cards — tap a button or just type the answer.
- Topics track state on their icon (custom-emoji ID, renders as 💻/💬/✅/❗️/🏁/❓ — card glyphs differ). Titles sync 1:1 with herdr pane names either way (rename the topic or the pane).

## Security

This is shell access over chat — treat it like SSH. Anyone holding your phone or your bot token owns the machine: guard both like keys. Owner-only by numeric id; no auth beyond that.

## Limitations

Hobby POC, not hardened: no audit log, plaintext local state, single instance. Needs network to Telegram (long-poll, no webhooks). One bot, one Herdr socket, one machine.

## License

MIT
