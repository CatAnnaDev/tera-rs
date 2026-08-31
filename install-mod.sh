#!/bin/sh
set -eu
crate="${1:?usage: ./install-mod.sh <crate> (ex: mod-example)}"
cargo build --release -p "$crate"
mkdir -p mods
lib="lib$(printf '%s' "$crate" | tr '-' '_').dylib"
cp "target/release/$lib" "mods/$lib"
echo "installe target/release/$lib -> mods/$lib"
