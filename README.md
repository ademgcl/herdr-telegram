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
| `TELEGRAM_CHAT_ID` | your numeric id ([@userinfobot](https://t.me/userinfobot); `TELEGRAM_CHAT_ID_*` adds co-owners) |
| `TELEGRAM_FORUM_CHAT_ID` | supergroup id, full negative form (omit for DM-only; alias `TELEGRAM_GROUP_CHAT_ID`) |
| `HERDR_SOCKET` | optional, default `~/.config/herdr/herdr.sock` |
| `HERDR_STATE_DIR` | optional, state dir (default: launch directory — all runs must share one) |
| `HERDR_TG_PORT` | optional, guard port (default `47319`) |

## Run

```sh
./target/release/herdr-telegram dev install  # write plist + bootstrap: auto-restart on crash/reboot, logs to bot.log
./target/release/herdr-telegram dev uninstall  # bootout + remove plist
launchctl bootout gui/$(id -u)/dev.herdr.telegram  # stop before manual runs — single instance only, else getUpdates Conflict (`dev install` to resume prod)
```

## Use

- General: `/agents` `/spawn <kind> [space]` `/space [name]` `/shell [space]` `/pane [space]` `/model` (use in topic) `/start` `/reset` (all topics) `/cancel` `/help` — topic/DM commands (`/quit` `/kill` `/split` `/read` `/output` `/status` `/keys`) redirect here with guidance.
- Agent topic: plain text = prompt. `/read [n]` `/output [n]` `/model` `/quit` `/kill` `/keys` `/status` `/shell` `/pane [space]` `/space` `/split` `/card` `/esc` `/cancel` `/reset` (this topic, bare only) `/history [n]` `/help` (+ `/start` shows help; `/agents` `/spawn` redirect to General/DM).
- Shell topic: plain text = shell input; `/esc` sends a raw Esc. No agent here, so `/card`, `/model` and `/quit` refuse; no `/shell` — use `/pane`; `/agents` `/spawn` redirect to General/DM; everything else matches the agent list (see `/help`).
- `/space [name]` creates a space and opens its shell topic — works in any topic.
- `/spawn opencode` in General opens its topic. DMs take the full set (reply picks the agent).
- No buttons and stuck? `/card` re-posts the card; `/esc` dismisses blocked dialogs.
- Topics carry a live kind icon (customs kept) and an identity card; titles sync either way, status never pins.

## Ops

`herdr-telegram dev` → console (rebuild on change, logs, topics, cleanup). One-shots: `dev status|logs|start|stop|restart|cleanup|build|install|uninstall`. Run prod on launchd; dev stays foreground.

## Security

Shell access over chat — treat it like SSH. Phone or bot token in hand means machine in hand. Owner-only by numeric id, nothing beyond that.

## Limitations

Hobby POC, not hardened: no audit log, plaintext local state, single instance. Needs network to Telegram. One bot, one Herdr socket, one machine.

## License

MIT
