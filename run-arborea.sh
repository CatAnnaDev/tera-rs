#!/bin/sh
set -e
root=$(cd "$(dirname "$0")" && pwd)
wine="/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine"
bottle="tera"
launcher="${ARBOREA_LAUNCHER:-/Users/anna/Downloads/ArboreaReborn_Launcher/launcher/Arborea Reborn.exe}"
server="${ARBOREA_SERVER:-31.204.136.149:7800}"
proxy_listen="127.0.0.1:9250"
dylib="$root/wine-redirect/libtera_redirect.dylib"
mods="$root/mods-live"

cargo build --release --manifest-path "$root/Cargo.toml" -p tera-proxy -p mod-radar
sh "$root/wine-redirect/build.sh"
mkdir -p "$mods"
cp "$root/target/release/libmod_radar.dylib" "$mods/"

"$root/target/release/tera-proxy" --listen "$proxy_listen" --upstream "$server" \
    --opcodes "$root/data/opcodes/protocol.376012.map" \
    --definitions "$root/data/definitions" \
    --mods-dir "$mods" --hide S_NPC_LOCATION --hide S_USER_LOCATION &
proxy=$!
trap 'kill $proxy 2>/dev/null' EXIT INT TERM
sleep 1

echo "proxy: $proxy_listen -> $server   (radar charge)"
echo "lancement du launcher Arborea avec redirection $server -> $proxy_listen"
echo "  -> si tu vois '[tera-redirect] actif', l'injection a pris"
echo

DYLD_INSERT_LIBRARIES="$dylib" \
TERA_REDIRECT_FROM="$server" \
TERA_REDIRECT_TO="$proxy_listen" \
    "$wine" --bottle "$bottle" "$launcher"
