#!/bin/sh
set -e
root=$(cd "$(dirname "$0")" && pwd)
bottle="$HOME/Library/Application Support/CrossOver/Bottles/Tera"
game="$bottle/drive_c/Games/TERA Europe Classic"
mods="$bottle/drive_c/users/crossover/AppData/Roaming/Crazy-eSports-ClassicPlus/mods"
opcodes="$root/data/opcodes/protocol.376012.map"
wine=/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine
temp="$bottle/drive_c/users/crossover/Temp"

cargo build --release --manifest-path "$root/Cargo.toml" -p tera-server
cargo build --release --manifest-path "$root/Cargo.toml" --target x86_64-pc-windows-gnu -p tera-launcher
cp "$root/target/x86_64-pc-windows-gnu/release/tera-launcher.exe" "$temp/"

"$root/target/release/tera-serverd" --opcodes "$opcodes" --definitions "$root/data/definitions" --database "$root/data/world.db" --auto-reply --hex &
server=$!
trap 'kill $server 2>/dev/null' EXIT INT TERM
sleep 1

"$wine" --bottle Tera --cx-app 'C:\users\crossover\Temp\tera-launcher.exe' -- \
    --account 35171 --host 127.0.0.1 --port 10001 --server-name Local \
    --game 'C:\Games\TERA Europe Classic\Binaries\TERA.exe'
