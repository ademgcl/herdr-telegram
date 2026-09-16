#!/usr/bin/env bash
# herdr-telegram interactive development, management and test script.
set -u
# Private logs/state by default (bot.log can carry chat IDs + excerpts).
umask 077
cd "$(dirname "$0")" || exit 1

BIN="./target/debug/herdr-telegram"
LOG_FILE="bot.log"
PID="" LAST_START=0 CRASH_COUNT=0 CRASH_PAUSED=0

# Colors
if [ -t 1 ]; then
  BOLD='\033[1m' DIM='\033[2m' RED='\033[0;31m' GREEN='\033[0;32m'
  YELLOW='\033[0;33m' CYAN='\033[0;36m' NC='\033[0m'
else
  BOLD='' DIM='' RED='' GREEN='' YELLOW='' CYAN='' NC=''
fi

# Load .env
if [ -f .env ]; then
  while IFS='=' read -r key val || [ -n "$key" ]; do
    [[ "$key" =~ ^#.*$ || -z "$key" ]] && continue
    key=$(echo "$key" | tr -d '[:space:]')
    val=$(echo "$val" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
    [ -n "$key" ] && [ -z "${!key:-}" ] && export "$key"="$val"
  done < .env
fi
PORT="${HERDR_TG_PORT:-47319}"

get_port_pid() { lsof -ti ":$PORT" 2>/dev/null | head -n 1; }
get_bot_pids() { pgrep -f "target/(debug|release)/herdr-telegram" 2>/dev/null || true; }
checksum() { find src Cargo.toml Cargo.lock -type f -exec stat -f '%m %N' {} + 2>/dev/null | sort | cksum; }

# Mask secrets/PII in displayed output (files untouched):
# home-dir user, bot tokens, Telegram chat IDs, TELEGRAM_* assignments.
mask() {
  local home="${HOME:-/nonexistent-home}"
  sed -e "s|$home|~|g" \
      -e 's|/Users/[^ /]*|/Users/***|g' \
      -e 's|/home/[^ /]*|/home/***|g' \
      -e 's|\(chat_id *= *\)-\?[0-9][0-9]*|\1***|g' \
      -e 's|-100[0-9][0-9]*|-100***|g' \
      -e 's|[0-9]\{6,\}:[A-Za-z0-9_-]\{20,\}|***TOKEN***|g' \
      -e 's|\(TELEGRAM_[A-Z_]*=\)[^ ]*|\1***|g'
}

stop_pid() {
  local target_pid="$1" timeout="${2:-5}"
  if [ -n "$target_pid" ] && kill -0 "$target_pid" 2>/dev/null; then
    kill -TERM "$target_pid" 2>/dev/null
    local i=0
    while kill -0 "$target_pid" 2>/dev/null && [ "$i" -lt "$timeout" ]; do sleep 1; i=$((i + 1)); done
    if kill -0 "$target_pid" 2>/dev/null; then
      kill -KILL "$target_pid" 2>/dev/null
      wait "$target_pid" 2>/dev/null
    fi
  fi
}

stop_bot() { [ -n "$PID" ] && stop_pid "$PID" 5; PID=""; }
cleanup_and_exit() { trap - INT TERM EXIT; echo -e "\n${CYAN}[dev] Shutting down...${NC}"; stop_bot; exit 0; }

cmd_ctl() {
  if [ ! -x "$BIN" ]; then cargo build || return 1; fi
  "$BIN" ctl "$@" | mask
}

cmd_cleanup() {
  echo -e "${YELLOW}[cleanup] Checking running bots and port ($PORT)...${NC}"
  local all_pids
  all_pids=$(echo "$(get_bot_pids) $(get_port_pid)" | tr ' ' '\n' | grep -v '^$' | sort -u)
  if [ -n "$all_pids" ]; then
    for p in $all_pids; do
      if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then
        echo -e "${YELLOW}[cleanup] Terminating PID $p...${NC}"
        stop_pid "$p" 5
      fi
    done
    echo -e "${GREEN}[cleanup] All instances cleared, port $PORT released.${NC}"
  else
    echo -e "${GREEN}[cleanup] No conflicting process found.${NC}"
  fi
}

cmd_status() {
  echo -e "${BOLD}${CYAN}=== HERDR TELEGRAM STATUS ===${NC}"
  local pids port_pid sock
  pids=$(get_bot_pids); port_pid=$(get_port_pid)
  if [ -n "$pids" ]; then
    echo -e "  Bot Status:   ${GREEN}Running${NC} (PID: $pids)"
    for p in $pids; do
      ps -o pid,pcpu,pmem,etime,command -p "$p" 2>/dev/null | tail -n +2 | while read -r line; do
        echo -e "                ${DIM}$line${NC}" | mask
      done
    done
  else
    echo -e "  Bot Status:   ${RED}Stopped${NC}"
  fi
  [ -n "$port_pid" ] && echo -e "  Guard Port:   ${YELLOW}In use :$PORT${NC} (PID: $port_pid)" || echo -e "  Guard Port:   ${GREEN}Free :$PORT${NC}"
  sock="${HERDR_SOCKET:-$HOME/.config/herdr/herdr.sock}"
  sock="${sock/#\~/$HOME}"
  [ -S "$sock" ] && echo -e "  Herdr Socket: ${GREEN}Available${NC} ($sock)" | mask || echo -e "  Herdr Socket: ${YELLOW}Not found${NC} ($sock)" | mask
  [ -f "offset.state" ] && echo -e "  Offset State: $(cat offset.state 2>/dev/null)"
  [ -f "topics.state" ] && echo -e "  Topics State: ~$(grep -c '":' topics.state 2>/dev/null || echo 0) titles"
  [ -f "$LOG_FILE" ] && echo -e "  Log File:     $LOG_FILE ($(ls -lh "$LOG_FILE" 2>/dev/null | awk '{print $5}'))"
  echo -e "${BOLD}${CYAN}=============================${NC}"
}

cmd_logs() {
  local lines="${1:-30}" follow="${2:-0}"
  [ ! -f "$LOG_FILE" ] && { echo -e "${YELLOW}$LOG_FILE not created yet.${NC}"; return; }
  if [ "$follow" -eq 1 ]; then
    echo -e "${CYAN}--- $LOG_FILE following live (Ctrl+C to exit) ---${NC}"
    tail -n "$lines" -f "$LOG_FILE" | mask
  else
    echo -e "${CYAN}--- $LOG_FILE last $lines lines ---${NC}"
    tail -n "$lines" "$LOG_FILE" | mask
    echo -e "${CYAN}---------------------------------${NC}"
  fi
}

start_bot() {
  local port_pid
  port_pid=$(get_port_pid)
  if [ -n "$port_pid" ] && [ "$port_pid" != "$PID" ]; then
    echo -e "${YELLOW}⚠️  Port $PORT in use (PID: $port_pid).${NC}"
    if [ -t 0 ]; then
      echo -n -e "${YELLOW}Stop current process (PID $port_pid) and start new bot? [Y/n]: ${NC}"
      read -t 5 -r ans || ans="y"; echo
      if [[ -z "$ans" || "$ans" =~ ^[yYeE] ]]; then
        stop_pid "$port_pid" 3
      else
        echo -e "${RED}[dev] Start cancelled: Port $PORT busy.${NC}"
        CRASH_PAUSED=1; return 1
      fi
    else
      stop_pid "$port_pid" 3
    fi
  fi

  echo -e "${CYAN}[dev] Compiling...${NC}"
  if ! cargo build 2>&1 | tail -n 4; then
    echo -e "${RED}[dev] Build FAILED — will retry automatically once code is fixed.${NC}"
    return 1
  fi

  "$BIN" >> "$LOG_FILE" 2>&1 < /dev/null &
  PID=$!; LAST_START=$(date +%s)
  echo -e "${GREEN}🟢 [dev] Bot started (PID: $PID)${NC}"
  show_quick_menu
}

show_quick_menu() {
  echo -e "${DIM}[r] Start | [b] Build | [t] Topics | [e] Event | [l] Log | [s] Status | [c] Clear | [q] Quit${NC}"
}

show_interactive_help() {
  echo -e "${BOLD}Interactive Shortcuts:${NC}
  ${CYAN}r${NC} : Rebuild and restart the bot now
  ${CYAN}b${NC} : Run 'cargo check'
  ${CYAN}t${NC} : Topic manager (list, single reset)
  ${CYAN}e${NC} : Event simulator (trigger blocked/working/done)
  ${CYAN}l${NC} : View recent logs (live follow option)
  ${CYAN}s${NC} : Detailed status table (PID, port, socket, state)
  ${CYAN}c${NC} : Clear $LOG_FILE
  ${CYAN}k${NC} : Stop bot | ${CYAN}x${NC} : Clear conflicts | ${CYAN}q${NC} : Quit"
}

check_health() {
  if [ -n "$PID" ] && ! kill -0 "$PID" 2>/dev/null; then
    local now diff port_pid
    now=$(date +%s); diff=$((now - LAST_START)); PID=""
    if [ "$diff" -lt 4 ]; then
      CRASH_COUNT=$((CRASH_COUNT + 1))
      echo -e "\n${RED}⚠️  [dev] Bot exited immediately (within ${diff}s)!${NC}\n${RED}--- Recent Logs ($LOG_FILE) ---${NC}"
      tail -n 8 "$LOG_FILE" 2>/dev/null | mask; echo -e "${RED}------------------------------${NC}"
      port_pid=$(get_port_pid)
      [ -n "$port_pid" ] && echo -e "${YELLOW}💡 Port $PORT busy (PID: $port_pid). Press [x] to clear it.${NC}"
      if [ "$CRASH_COUNT" -ge 3 ]; then
        echo -e "${RED}🚨 [dev] $CRASH_COUNT consecutive crashes! Auto-restart paused.${NC}"
        echo -e "${CYAN}👉 [r] Retry | [x] Clear conflicts | [b] Build | [q] Quit${NC}"
        CRASH_PAUSED=1; return 1
      fi
    else
      CRASH_COUNT=0; echo -e "\n${YELLOW}[dev] Bot stopped.${NC}"
    fi
    [ "$CRASH_PAUSED" -eq 0 ] && { echo -e "${CYAN}[dev] Restarting...${NC}"; start_bot; }
  fi
}

cmd_dev() {
  trap cleanup_and_exit INT TERM EXIT
  echo -e "${BOLD}${CYAN}╭──────────────────────────────────────────────────╮${NC}"
  echo -e "${BOLD}${CYAN}│        herdr-telegram Interactive Dev Console    │${NC}"
  echo -e "${BOLD}${CYAN}│        Watching: src/, Cargo.toml                │${NC}"
  echo -e "${BOLD}${CYAN}╰──────────────────────────────────────────────────╯${NC}"
  echo -e "${DIM}Press [h] for shortcut help.${NC}\n"

  start_bot
  local prev cur key
  prev=$(checksum)

  while true; do
    check_health
    cur=$(checksum)
    if [ "$cur" != "$prev" ]; then
      prev="$cur"
      echo -e "\n${CYAN}⚡ [dev] Change detected, rebuilding...${NC}"
      CRASH_PAUSED=0; CRASH_COUNT=0; sleep 0.5; stop_bot; start_bot
      prev=$(checksum); continue
    fi

    key=""
    if read -t 1 -n 1 key 2>/dev/null; then
      case "$key" in
        r|R) echo -e "\n${CYAN}[dev] Restarting...${NC}"; CRASH_PAUSED=0; CRASH_COUNT=0; stop_bot; start_bot; prev=$(checksum) ;;
        b|B) echo -e "\n${CYAN}[dev] Running cargo check...${NC}"; cargo check; show_quick_menu ;;
        t|T)
          echo -e "\n${CYAN}--- Topic Management ---${NC}"; cmd_ctl topics
          echo -n -e "${YELLOW}Pane/topic ID to reset (empty = cancel): ${NC}"; read -r t_tar
          [ -n "$t_tar" ] && cmd_ctl reset "$t_tar"; show_quick_menu ;;
        e|E)
          echo -e "\n${CYAN}--- Event Simulator ---${NC}"
          echo -n -e "${YELLOW}Pane name (e.g. w1:p2): ${NC}"; read -r e_p
          if [ -n "$e_p" ]; then
            echo -n -e "${YELLOW}Status [blocked|working|done|idle]: ${NC}"; read -r e_s
            [ -n "$e_s" ] && cmd_ctl trigger "$e_p" "$e_s"
          fi; show_quick_menu ;;
        l|L)
          echo ""; cmd_logs 25 0; echo -n -e "${YELLOW}Follow live (tail -f)? [y/N]: ${NC}"
          read -t 5 -r ans || ans="n"; echo; [[ "$ans" =~ ^[yYeE] ]] && cmd_logs 25 1; show_quick_menu ;;
        s|S) echo ""; cmd_status; show_quick_menu ;;
        c|C) echo -e "\n${CYAN}[dev] $LOG_FILE cleared.${NC}"; : > "$LOG_FILE" ;;
        k|K) echo -e "\n${YELLOW}[dev] Bot stopped.${NC}"; stop_bot; CRASH_PAUSED=1 ;;
        x|X) echo -e "\n${YELLOW}[dev] Cleaning up...${NC}"; cmd_cleanup; CRASH_PAUSED=0; CRASH_COUNT=0; start_bot; prev=$(checksum) ;;
        h|H|\?) echo ""; show_interactive_help; show_quick_menu ;;
        q|Q) cleanup_and_exit ;;
      esac
    fi
  done
}

show_cli_help() {
  echo -e "${BOLD}Usage:${NC} ./dev.sh [COMMAND] [ARGS...]
${BOLD}Development & Control:${NC}
  ${CYAN}dev${NC}                      : Interactive monitoring mode (watcher + shortcuts)
  ${CYAN}help${NC} (default, empty)    : This help
  ${CYAN}topics${NC}                  : Active tabs and topic sync table
  ${CYAN}reset${NC} <pane|#topic>     : Delete and recreate a single pane/topic
  ${CYAN}trigger${NC} <pane> <status> : Fire a mock status event (blocked, working, done)
  ${CYAN}inspect${NC} <pane>          : Show details of a specific pane
  ${CYAN}cleanup${NC}                 : Clear zombie bots and guard port
  ${CYAN}status${NC}                  : Show bot, guard port and socket status
  ${CYAN}logs${NC} [-f]               : Follow the bot.log file
  ${CYAN}start${NC} | ${CYAN}stop${NC} | ${CYAN}restart${NC}  : Background service controls
  ${CYAN}build${NC} | ${CYAN}check${NC}          : Cargo build / check commands"
}

case "${1:-help}" in
  dev|watch|run) cmd_dev ;;
  topics) shift; cmd_ctl topics "$@" ;;
  reset) shift; cmd_ctl reset "$@" ;;
  trigger) shift; cmd_ctl trigger "$@" ;;
  inspect) shift; cmd_ctl inspect "$@" ;;
  cleanup|clean) cmd_cleanup ;;
  status) cmd_status ;;
  logs|log|tail) cmd_logs "${2:-50}" 1 ;;
  start) cmd_cleanup; start_bot ;;
  stop) cmd_cleanup ;;
  restart) cmd_cleanup; start_bot ;;
  build) cargo build ;;
  check) cargo check ;;
  help|-h|--help|"") show_cli_help ;;
  *) echo -e "${RED}Unknown command: $1${NC}"; show_cli_help; exit 1 ;;
esac
