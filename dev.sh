#!/usr/bin/env bash
# herdr-telegram interaktif geliştirme, yönetim ve test betiği.
set -u
cd "$(dirname "$0")" || exit 1

BIN="./target/debug/herdr-telegram"
LOG_FILE="bot.log"
PID="" LAST_START=0 CRASH_COUNT=0 CRASH_PAUSED=0

# Renkler
if [ -t 1 ]; then
  BOLD='\033[1m' DIM='\033[2m' RED='\033[0;31m' GREEN='\033[0;32m'
  YELLOW='\033[0;33m' CYAN='\033[0;36m' NC='\033[0m'
else
  BOLD='' DIM='' RED='' GREEN='' YELLOW='' CYAN='' NC=''
fi

# .env yükle
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
cleanup_and_exit() { trap - INT TERM EXIT; echo -e "\n${CYAN}[dev] Kapatılıyor...${NC}"; stop_bot; exit 0; }

cmd_ctl() {
  if [ ! -x "$BIN" ]; then cargo build || return 1; fi
  "$BIN" ctl "$@"
}

cmd_cleanup() {
  echo -e "${YELLOW}[cleanup] Çalışan botlar ve port ($PORT) kontrol ediliyor...${NC}"
  local all_pids
  all_pids=$(echo "$(get_bot_pids) $(get_port_pid)" | tr ' ' '\n' | grep -v '^$' | sort -u)
  if [ -n "$all_pids" ]; then
    for p in $all_pids; do
      if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then
        echo -e "${YELLOW}[cleanup] PID $p sonlandırılıyor...${NC}"
        stop_pid "$p" 5
      fi
    done
    echo -e "${GREEN}[cleanup] Tüm instancelar temizlendi, port $PORT boşaltıldı.${NC}"
  else
    echo -e "${GREEN}[cleanup] Çalışan çakışan süreç bulunamadı.${NC}"
  fi
}

cmd_status() {
  echo -e "${BOLD}${CYAN}=== HERDR TELEGRAM DURUMU ===${NC}"
  local pids port_pid sock
  pids=$(get_bot_pids); port_pid=$(get_port_pid)
  if [ -n "$pids" ]; then
    echo -e "  Bot Durumu:   ${GREEN}Çalışıyor${NC} (PID: $pids)"
    for p in $pids; do
      ps -o pid,pcpu,pmem,etime,command -p "$p" 2>/dev/null | tail -n +2 | while read -r line; do
        echo -e "                ${DIM}$line${NC}"
      done
    done
  else
    echo -e "  Bot Durumu:   ${RED}Durduruldu${NC}"
  fi
  [ -n "$port_pid" ] && echo -e "  Guard Port:   ${YELLOW}Kullanımda :$PORT${NC} (PID: $port_pid)" || echo -e "  Guard Port:   ${GREEN}Boş :$PORT${NC}"
  sock="${HERDR_SOCKET:-$HOME/.config/herdr/herdr.sock}"
  sock="${sock/#\~/$HOME}"
  [ -S "$sock" ] && echo -e "  Herdr Soketi: ${GREEN}Mevcut${NC} ($sock)" || echo -e "  Herdr Soketi: ${YELLOW}Bulunamadı${NC} ($sock)"
  [ -f "offset.state" ] && echo -e "  Offset State: $(cat offset.state 2>/dev/null)"
  [ -f "topics.state" ] && echo -e "  Topics State: ~$(grep -c '":' topics.state 2>/dev/null || echo 0) başlık"
  [ -f "$LOG_FILE" ] && echo -e "  Log Dosyası:  $LOG_FILE ($(ls -lh "$LOG_FILE" 2>/dev/null | awk '{print $5}'))"
  echo -e "${BOLD}${CYAN}=============================${NC}"
}

cmd_logs() {
  local lines="${1:-30}" follow="${2:-0}"
  [ ! -f "$LOG_FILE" ] && { echo -e "${YELLOW}Henüz $LOG_FILE oluşmamış.${NC}"; return; }
  if [ "$follow" -eq 1 ]; then
    echo -e "${CYAN}--- $LOG_FILE canlı izleniyor (Çıkmak için Ctrl+C) ---${NC}"
    tail -n "$lines" -f "$LOG_FILE"
  else
    echo -e "${CYAN}--- $LOG_FILE son $lines satır ---${NC}"
    tail -n "$lines" "$LOG_FILE"
    echo -e "${CYAN}---------------------------------${NC}"
  fi
}

start_bot() {
  local port_pid
  port_pid=$(get_port_pid)
  if [ -n "$port_pid" ] && [ "$port_pid" != "$PID" ]; then
    echo -e "${YELLOW}⚠️  Port $PORT kullanımda (PID: $port_pid).${NC}"
    if [ -t 0 ]; then
      echo -n -e "${YELLOW}Mevcut süreci (PID $port_pid) durdurup yeni botu başlatmak ister misiniz? [Y/n]: ${NC}"
      read -t 5 -r ans || ans="y"; echo
      if [[ -z "$ans" || "$ans" =~ ^[yYeE] ]]; then
        stop_pid "$port_pid" 3
      else
        echo -e "${RED}[dev] Başlatma iptal edildi: Port $PORT meşgul.${NC}"
        CRASH_PAUSED=1; return 1
      fi
    else
      stop_pid "$port_pid" 3
    fi
  fi

  echo -e "${CYAN}[dev] Derleniyor...${NC}"
  if ! cargo build 2>&1 | tail -n 4; then
    echo -e "${RED}[dev] Derleme BAŞARISIZ — kod düzeltildiğinde otomatik tekrar denenecek.${NC}"
    return 1
  fi

  "$BIN" >> "$LOG_FILE" 2>&1 < /dev/null &
  PID=$!; LAST_START=$(date +%s)
  echo -e "${GREEN}🟢 [dev] Bot başlatıldı (PID: $PID)${NC}"
  show_quick_menu
}

show_quick_menu() {
  echo -e "${DIM}[r] Başlat | [b] Derle | [t] Topics | [e] Event | [l] Log | [s] Durum | [c] Temizle | [q] Çık${NC}"
}

show_interactive_help() {
  echo -e "${BOLD}İnteraktif Kısayollar:${NC}
  ${CYAN}r${NC} : Botu hemen yeniden derle ve başlat
  ${CYAN}b${NC} : 'cargo check' çalıştır
  ${CYAN}t${NC} : Topic yöneticisi (listele, tekil reset)
  ${CYAN}e${NC} : Event simülatörü (blocked/working/done tetikle)
  ${CYAN}l${NC} : Son logları görüntüle (canlı izleme seçeneği)
  ${CYAN}s${NC} : Detaylı durum tablosu (PID, port, soket, state)
  ${CYAN}c${NC} : $LOG_FILE dosyasını temizle
  ${CYAN}k${NC} : Botu durdur | ${CYAN}x${NC} : Çakışmaları temizle | ${CYAN}q${NC} : Çık"
}

check_health() {
  if [ -n "$PID" ] && ! kill -0 "$PID" 2>/dev/null; then
    local now diff port_pid
    now=$(date +%s); diff=$((now - LAST_START)); PID=""
    if [ "$diff" -lt 4 ]; then
      CRASH_COUNT=$((CRASH_COUNT + 1))
      echo -e "\n${RED}⚠️  [dev] Bot hemen kapandı (${diff}sn içinde)!${NC}\n${RED}--- Son Loglar ($LOG_FILE) ---${NC}"
      tail -n 8 "$LOG_FILE" 2>/dev/null; echo -e "${RED}------------------------------${NC}"
      port_pid=$(get_port_pid)
      [ -n "$port_pid" ] && echo -e "${YELLOW}💡 Port $PORT dolu (PID: $port_pid). [x] tuşuna basarak temizleyebilirsiniz.${NC}"
      if [ "$CRASH_COUNT" -ge 3 ]; then
        echo -e "${RED}🚨 [dev] Peş peşe $CRASH_COUNT crash! Otomatik başlatma duraklatıldı.${NC}"
        echo -e "${CYAN}👉 [r] Tekrar dene | [x] Çakışmaları temizle | [b] Derle | [q] Çık${NC}"
        CRASH_PAUSED=1; return 1
      fi
    else
      CRASH_COUNT=0; echo -e "\n${YELLOW}[dev] Bot durdu.${NC}"
    fi
    [ "$CRASH_PAUSED" -eq 0 ] && { echo -e "${CYAN}[dev] Yeniden başlatılıyor...${NC}"; start_bot; }
  fi
}

cmd_dev() {
  trap cleanup_and_exit INT TERM EXIT
  echo -e "${BOLD}${CYAN}╭──────────────────────────────────────────────────╮${NC}"
  echo -e "${BOLD}${CYAN}│     herdr-telegram İnteraktif Dev Konsolu        │${NC}"
  echo -e "${BOLD}${CYAN}│     İzleniyor: src/, Cargo.toml                  │${NC}"
  echo -e "${BOLD}${CYAN}╰──────────────────────────────────────────────────╯${NC}"
  echo -e "${DIM}Kısayol yardımı için [h] tuşuna basın.${NC}\n"

  start_bot
  local prev cur key
  prev=$(checksum)

  while true; do
    check_health
    cur=$(checksum)
    if [ "$cur" != "$prev" ]; then
      prev="$cur"
      echo -e "\n${CYAN}⚡ [dev] Değişiklik algılandı, yeniden derleniyor...${NC}"
      CRASH_PAUSED=0; CRASH_COUNT=0; sleep 0.5; stop_bot; start_bot
      prev=$(checksum); continue
    fi

    key=""
    if read -t 1 -n 1 key 2>/dev/null; then
      case "$key" in
        r|R) echo -e "\n${CYAN}[dev] Yeniden başlatılıyor...${NC}"; CRASH_PAUSED=0; CRASH_COUNT=0; stop_bot; start_bot; prev=$(checksum) ;;
        b|B) echo -e "\n${CYAN}[dev] cargo check çalıştırılıyor...${NC}"; cargo check; show_quick_menu ;;
        t|T)
          echo -e "\n${CYAN}--- Topic Yönetimi ---${NC}"; cmd_ctl topics
          echo -n -e "${YELLOW}Resetlenecek pane/topic ID (boş = iptal): ${NC}"; read -r t_tar
          [ -n "$t_tar" ] && cmd_ctl reset "$t_tar"; show_quick_menu ;;
        e|E)
          echo -e "\n${CYAN}--- Event Simülatörü ---${NC}"
          echo -n -e "${YELLOW}Pane adı (örn. w1:p2): ${NC}"; read -r e_p
          if [ -n "$e_p" ]; then
            echo -n -e "${YELLOW}Durum [blocked|working|done|idle]: ${NC}"; read -r e_s
            [ -n "$e_s" ] && cmd_ctl trigger "$e_p" "$e_s"
          fi; show_quick_menu ;;
        l|L)
          echo ""; cmd_logs 25 0; echo -n -e "${YELLOW}Canlı takip (tail -f)? [y/N]: ${NC}"
          read -t 5 -r ans || ans="n"; echo; [[ "$ans" =~ ^[yYeE] ]] && cmd_logs 25 1; show_quick_menu ;;
        s|S) echo ""; cmd_status; show_quick_menu ;;
        c|C) echo -e "\n${CYAN}[dev] $LOG_FILE temizlendi.${NC}"; : > "$LOG_FILE" ;;
        k|K) echo -e "\n${YELLOW}[dev] Bot durduruldu.${NC}"; stop_bot; CRASH_PAUSED=1 ;;
        x|X) echo -e "\n${YELLOW}[dev] Temizleniyor...${NC}"; cmd_cleanup; CRASH_PAUSED=0; CRASH_COUNT=0; start_bot; prev=$(checksum) ;;
        h|H|\?) echo ""; show_interactive_help; show_quick_menu ;;
        q|Q) cleanup_and_exit ;;
      esac
    fi
  done
}

show_cli_help() {
  echo -e "${BOLD}Kullanım:${NC} ./dev.sh [KOMUT] [ARGÜMANLAR...]
${BOLD}Geliştirme & Kontrol:${NC}
  ${CYAN}dev${NC}                      : İnteraktif izleme modu (watcher + kısayollar)
  ${CYAN}help${NC} (varsayılan, boş)   : Bu yardım
  ${CYAN}topics${NC}                  : Aktif tab'ler ve topic senkronizasyon tablosu
  ${CYAN}reset${NC} <pane|#topic>     : Tek bir pane/topic'i silip yeniden oluştur
  ${CYAN}trigger${NC} <pane> <status> : Mock durum olayı fırlat (blocked, working, done)
  ${CYAN}inspect${NC} <pane>          : Belirli bir pane'in detaylarını göster
  ${CYAN}cleanup${NC}                 : Zombi botları ve guard portu temizle
  ${CYAN}status${NC}                  : Bot, guard port ve soket durumunu göster
  ${CYAN}logs${NC} [-f]               : bot.log dosyasını takip et
  ${CYAN}start${NC} | ${CYAN}stop${NC} | ${CYAN}restart${NC}  : Arka plan servis kontrolleri
  ${CYAN}build${NC} | ${CYAN}check${NC}          : Cargo derleme / kontrol komutları"
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
  *) echo -e "${RED}Bilinmeyen komut: $1${NC}"; show_cli_help; exit 1 ;;
esac
