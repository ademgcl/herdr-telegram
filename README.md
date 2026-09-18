# herdr-telegram

You're away from the desk when an agent stalls on a question. Telegram buzzes with the prompt and answer buttons — you tap one, the run continues. Start on desktop, finish on the phone.

- One topic per pane: plain text is the prompt, titles sync 1:1 with Herdr.
- Blocked agents post answer cards: tap a button or type the answer.

## You need

- [Herdr](https://herdr.dev) running agents on this machine
- A bot token: [@BotFather](https://t.me/BotFather) → `/newbot`
- A forum supergroup: new group → enable Topics → bot as **admin**. One group per machine keeps hosts apart.

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
| `TELEGRAM_FORUM_CHAT_ID` | supergroup id, full negative form (omit for DM-only) |
| `HERDR_SOCKET` | optional, default `~/.config/herdr/herdr.sock` |

## Run

```sh
sed "s|__REPO_DIR__|$(pwd)|g" dev.herdr.telegram.plist.example > ~/Library/LaunchAgents/dev.herdr.telegram.plist && launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.herdr.telegram.plist  # auto-restart on crash/reboot, logs to bot.log
launchctl bootout gui/$(id -u)/dev.herdr.telegram  # stop before manual runs — single instance only, else getUpdates Conflict
launchctl bootout gui/$(id -u)/dev.herdr.telegram; rm ~/Library/LaunchAgents/dev.herdr.telegram.plist  # uninstall
```

## Use

- General: `/agents` `/spawn <kind> [space]` `/space [name]` `/shell [space]` `/pane [space]` `/model` (use in topic) `/start` `/reset` (all topics) `/cancel` `/help`
- Agent topic: plain text = prompt. `/read` `/output` `/model` `/quit` `/kill` `/keys` `/status` `/shell` `/pane [space]` `/space` `/split` `/card` `/esc` `/cancel` `/reset` (this topic) `/help`
- Shell topic: plain text = shell input; `/esc` sends a raw Esc. No agent here, so `/card`, `/model` and `/quit` refuse. No `/shell` — use `/pane`; everything else matches the agent list (see `/help`).
- `/space [name]` creates a space and opens its shell topic — works in any topic.
- `/spawn opencode` in General opens its topic. DMs work too (reply picks the agent).
- No buttons and stuck? `/card` re-posts the card; `/esc` dismisses blocked dialogs.
- Topics carry a live kind icon (customs kept) and an identity card; titles sync either way, status never pins.

## Ops

`herdr-telegram dev` → console (rebuild on change, logs, topics, cleanup). One-shots: `dev status|logs|start|stop|restart|cleanup|build`. Run prod on launchd; dev stays foreground.

## Security

Shell access over chat — treat it like SSH. Phone or bot token in hand means machine in hand. Owner-only by numeric id, nothing beyond that.

## Limitations

Hobby POC, not hardened: no audit log, plaintext local state, single instance. Needs network to Telegram. One bot, one Herdr socket, one machine.

## License

MIT
