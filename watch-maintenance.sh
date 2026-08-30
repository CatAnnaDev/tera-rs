#!/bin/sh
set -eu

HASH_URL="https://s2.tera-europe.net/client/hash-file.json"
LIST_URL="https://launcher.tera-europe.net/classicplus/serverlist.json"
INTERVAL="${1:-300}"
UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

sig_hash() { curl -4 -sSI --max-time 15 -A "$UA" "$HASH_URL" | tr -d '\r' | awk 'tolower($1)=="last-modified:"||tolower($1)=="content-length:"{print}'; }
sig_list() { curl -4 -sS  --max-time 15 -A "$UA" "$LIST_URL"; }

notify() {
  osascript -e "display notification \"$2\" with title \"$1\" sound name \"Glass\"" >/dev/null 2>&1 || true
  printf '\a'
}

base_h="$(sig_hash)"; base_l="$(sig_list)"
echo "[watch] baseline :"
echo "$base_h" | sed 's/^/    hash-file  /'
echo "[watch] poll toutes les ${INTERVAL}s, HTTP only, jamais le port 9000. Ctrl+C pour stopper."
echo "[watch] astuce : decommente le 'break' dans le script pour t'arreter au 1er changement."

while :; do
  sleep "$INTERVAL"
  now_h="$(sig_hash)" || { echo "[$(date '+%H:%M:%S')] curl KO, on reessaie"; continue; }
  now_l="$(sig_list)" || { echo "[$(date '+%H:%M:%S')] curl KO, on reessaie"; continue; }
  changed=""
  [ "$now_h" != "$base_h" ] && changed="hash-file (patch client)"
  [ "$now_l" != "$base_l" ] && changed="${changed:+$changed + }serverlist"
  ts="$(date '+%H:%M:%S')"
  if [ -n "$changed" ]; then
    echo "[$ts] >>> CHANGE: $changed"
    echo "$now_h" | sed 's/^/    hash-file  /'
    notify "TERA bouge cote serveur" "$changed a change - maintenance sans doute finie ou en cours de fin"
    base_h="$now_h"; base_l="$now_l"
    # break
  else
    echo "[$ts] rien (toujours en maintenance / stable)"
  fi
done
