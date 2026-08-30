#!/usr/bin/env bash
set -euo pipefail

PINNED_TAG="GE-Proton11-6"
STEAM_ROOT="${HOME}/.local/share/Steam"
COMPAT_DIR="${STEAM_ROOT}/compatibilitytools.d"
GAME_DIR="${HOME}/Games/Tera-Europe-Classic"
PFX="${HOME}/.wine-Tera"

say() { printf '\n\033[1m=== %s ===\033[0m\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

require_arch() {
  have pacman || { echo "pacman absent : ce script est pour CachyOS/Arch. Stop."; exit 1; }
}

resolve_ge_tag() {
  local tag=""
  if have python3; then
    tag="$(curl -fsSL --max-time 15 \
      "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases?per_page=20" 2>/dev/null \
      | python3 -c 'import json,sys
try:
    rel=json.load(sys.stdin)
    c=[r["tag_name"] for r in rel if r.get("tag_name","").startswith("GE-Proton11")]
    print(c[0] if c else "")
except Exception:
    print("")' 2>/dev/null || true)"
  fi
  [ -n "$tag" ] && echo "$tag" || echo "$PINNED_TAG"
}

enable_multilib() {
  if grep -q '^\[multilib\]' /etc/pacman.conf; then
    echo "multilib deja actif."
  else
    say "Activation du repo multilib"
    sudo sed -i '/\[multilib\]/,/Include/ s/^#//' /etc/pacman.conf
  fi
}

install_steam() {
  say "Installation Steam + rendu Vulkan logiciel (VM sans GPU)"
  sudo pacman -Syu --needed --noconfirm \
    steam \
    mesa lib32-mesa \
    vulkan-swrast lib32-vulkan-swrast \
    vulkan-icd-loader lib32-vulkan-icd-loader \
    winetricks curl tar
}

fetch_geproton() {
  local tag="$1"
  local asset="${tag}-x86_64.tar.gz"
  local base="https://github.com/GloriousEggroll/proton-ge-custom/releases/download/${tag}"
  say "GE-Proton : ${tag} (x86_64)"
  mkdir -p "$COMPAT_DIR"
  if [ -d "${COMPAT_DIR}/${tag}" ]; then
    echo "${tag} deja present, on saute le telechargement."
    return
  fi
  local tmp
  tmp="$(mktemp -d)"
  echo "Telechargement de ${asset} ..."
  curl -L --fail --progress-bar -o "${tmp}/${asset}" "${base}/${asset}"
  if curl -L --fail -sS -o "${tmp}/sum" "${base}/${tag}-x86_64.sha512sum"; then
    say "Verification sha512"
    ( cd "$tmp" && sha512sum -c <(awk -v f="$asset" '{print $1"  "f}' sum) )
  else
    echo "sha512sum indisponible, extraction sans verif."
  fi
  say "Extraction vers ${COMPAT_DIR}"
  tar -xf "${tmp}/${asset}" -C "$COMPAT_DIR"
  rm -rf "$tmp"
  if [ -d "${HOME}/.steam/root" ]; then
    mkdir -p "${HOME}/.steam/root/compatibilitytools.d"
    ln -sfn "${COMPAT_DIR}/${tag}" "${HOME}/.steam/root/compatibilitytools.d/${tag}"
  fi
  echo "${tag} installe dans ${COMPAT_DIR}."
}

make_prefix() {
  local tag="$1"
  local ge="${COMPAT_DIR}/${tag}"
  local wine="${ge}/files/bin/wine"
  local wineserver="${ge}/files/bin/wineserver"
  say "Prefix Wine (wine de GE-Proton) -> ${PFX}"
  [ -x "$wine" ] || { echo "wine introuvable dans ${ge}/files/bin. Stop."; exit 1; }
  mkdir -p "$PFX" "$GAME_DIR"
  if [ -f "${PFX}/system.reg" ]; then
    echo "prefix deja initialise, on saute."
  else
    echo "Initialisation du prefix (wineboot)..."
    WINEPREFIX="$PFX" WINEDEBUG=-all "$wine" wineboot --init
    WINEPREFIX="$PFX" "$wineserver" -w
  fi
  echo "prefix pret : ${PFX}"
}

next_steps() {
  local tag="$1"
  say "Termine"
  cat <<NEXT
Ce que tu as maintenant :
  - ${tag} dans ${COMPAT_DIR}
  - prefix Wine pret : ${PFX}
  - dossier jeu : ${GAME_DIR} (pose les fichiers TERA dedans)

Lancer, avec le recipe (dinput8=n,b pour Noctenium, fsync) :
  ./play-tera.sh launcher.exe      (ou TERA.exe selon ce que tu lances)

play-tera.sh fait exactement ton script Linux : WINEPREFIX=${PFX}, wine de ${tag},
WINEDLLOVERRIDES=dinput8=n,b, WINEFSYNC=1. Recupere-le sur le meme serveur que ce script.

Deps eventuelles dans le prefix (le wine GE embarque deja mono) :
  WINEPREFIX="${PFX}" winetricks -q corefonts d3dx9
  (le launcher officiel veut WebView2 ; le client seul non.)

Rappel : cette VM sort par ta Bouygues -> meme mur reseau. Le point ouvert reste l'egress (4G).
NEXT
}

main() {
  require_arch
  local tag
  tag="$(resolve_ge_tag)"
  enable_multilib
  install_steam
  fetch_geproton "$tag"
  make_prefix "$tag"
  next_steps "$tag"
}

main "$@"
