#!/bin/sh
set -e
root=$(cd "$(dirname "$0")" && pwd)
bottle="$HOME/Library/Application Support/CrossOver/Bottles/Tera"
temp="$bottle/drive_c/users/crossover/Temp"
wine=/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine
opcodes="$root/data/opcodes/protocol.376012.map"

if [ -z "${TERA_UPSTREAM:-}" ] || [ -z "${TERA_TICKET:-}" ]; then
    echo "usage: TERA_UPSTREAM=host:port TERA_TICKET=<ticket> [TERA_ACCOUNT=n] $0"
    echo
    echo "  TERA_UPSTREAM : adresse du vrai serveur (host:port)"
    echo "  TERA_TICKET   : ticket de connexion reel (AuthKey)"
    echo "  TERA_ACCOUNT  : numero de compte (defaut 35171)"
    echo
    echo "les deux se recuperent en lancant ./capture-real.sh (dumps reply_*.bin :"
    echo "account, ticket, puis serverlist qui contient l'adresse du serveur),"
    echo "ou via l'auth OAuth de tera-bot (tera-bot-auth.json -> auth_key)."
    exit 1
fi

account="${TERA_ACCOUNT:-35171}"
listen="127.0.0.1:9250"
mods="$root/mods"

cargo build --release --manifest-path "$root/Cargo.toml" -p tera-proxy -p mod-radar
cargo build --release --manifest-path "$root/Cargo.toml" --target x86_64-pc-windows-gnu -p tera-launcher
cp "$root/target/x86_64-pc-windows-gnu/release/tera-launcher.exe" "$temp/"

mkdir -p "$mods"
cp "$root/target/release/libmod_radar.dylib" "$mods/"

"$root/target/release/tera-proxy" --listen "$listen" --upstream "$TERA_UPSTREAM" \
    --opcodes "$opcodes" --definitions "$root/data/definitions" --mods-dir "$mods" &
proxy=$!
trap 'kill $proxy 2>/dev/null' EXIT INT TERM
sleep 1

echo "proxy: $listen -> $TERA_UPSTREAM   (mods: $mods)"
echo "lancement du client via le launcher stub..."

"$wine" --bottle Tera --cx-app 'C:\users\crossover\Temp\tera-launcher.exe' -- \
    --account "$account" --ticket "$TERA_TICKET" --host 127.0.0.1 --port 9250 \
    --server-name "Meow(proxy)" \
    --game 'C:\Games\TERA Europe Classic\Binaries\TERA.exe'
