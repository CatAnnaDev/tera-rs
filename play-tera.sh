#!/usr/bin/env bash
set -euo pipefail

GAME_DIR="${HOME}/Games/Tera-Europe-Classic"
export WINEPREFIX="${HOME}/.wine-Tera"

GE="$(ls -d "${HOME}/.local/share/Steam/compatibilitytools.d/GE-Proton11-"* 2>/dev/null | sort -V | tail -1 || true)"
if [ -n "${GE}" ] && [ -x "${GE}/files/bin/wine" ]; then
  WINE="${GE}/files/bin/wine"
else
  WINE="$(command -v wine || true)"
fi
[ -n "${WINE}" ] || { echo "aucun wine trouve (ni GE-Proton ni systeme)."; exit 1; }

export WINEDEBUG=-all
export WINEDLLOVERRIDES="dinput8=n,b"
export WINEFSYNC=1
export WINEESYNC=1
export PROTON_FORCE_LARGE_ADDRESS_AWARE=1

TARGET="${1:-}"
if [ -z "${TARGET}" ]; then
  echo "usage: $0 <exe> [args]"
  echo "  ex: $0 launcher.exe"
  echo "      $0 TERA.exe"
  echo "wine utilise : ${WINE}"
  exit 1
fi

cd "${GAME_DIR}"
echo "wine   : ${WINE}"
echo "prefix : ${WINEPREFIX}"
echo "jeu    : ${GAME_DIR}/${TARGET}"
exec "${WINE}" "${TARGET}" "${@:2}"
