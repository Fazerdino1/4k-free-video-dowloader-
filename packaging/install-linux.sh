#!/usr/bin/env bash
# Universal Linux installer / dependency & build orchestrator for 4k free video dowloader (vidgrab).
# Supports Debian/Ubuntu/Mint/Pop/Zorin/Elementary/Kali/Parrot/Tails/MX/RPi OS,
# Fedora/RHEL/Alma/Rocky/Nobara/Bazzite, openSUSE Leap/Tumbleweed,
# Arch/CachyOS/EndeavourOS/Manjaro/Garuda/SteamOS, Alpine (musl), Gentoo, Void,
# NixOS, GNU Guix, Solus, Slackware, Qubes OS, plus Flatpak / Snap / AppImage.
set -euo pipefail

DRY_RUN=0
CLI_ONLY=0
AUTO_YES=0

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --cli-only) CLI_ONLY=1 ;;
    --yes|-y) AUTO_YES=1 ;;
    --help|-h)
      cat <<'EOF'
Usage: ./packaging/install-linux.sh [OPTIONS]
  --dry-run    Affiche les commandes détectées pour votre distribution sans exécuter
  --cli-only   Installe uniquement vidgrab-cli (sans GTK/WebKitGTK : serveurs, RPi Lite, Alpine)
  -y, --yes    Accepte automatiquement les invites du gestionnaire de paquets
  -h, --help   Affiche cette aide
EOF
      exit 0
      ;;
    *)
      echo "Option inconnue : $arg" >&2
      exit 2
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

run_cmd() {
  if [ "$DRY_RUN" = "1" ]; then
    echo "[DRY-RUN] $*"
  else
    echo "+ $*"
    "$@"
  fi
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    run_cmd "$@"
  elif command -v sudo >/dev/null 2>&1; then
    run_cmd sudo "$@"
  elif command -v doas >/dev/null 2>&1; then
    run_cmd doas "$@"
  else
    echo "Droits root requis pour : $*" >&2
    exit 1
  fi
}

OS_ID="linux"
OS_LIKE=""
OS_NAME="Linux"
if [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  OS_ID="${ID:-linux}"
  OS_LIKE="${ID_LIKE:-}"
  OS_NAME="${PRETTY_NAME:-${NAME:-Linux}}"
fi
ARCH="$(uname -m)"

echo "Système détecté : $OS_NAME (id=$OS_ID, like=$OS_LIKE, arch=$ARCH)"

if [ "$CLI_ONLY" = "0" ] && [ -z "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; then
  echo "Aucun serveur graphique détecté : bascule automatique en --cli-only (vidgrab-cli)."
  CLI_ONLY=1
fi

# NixOS declarative flake
if [ -e /etc/NIXOS ] && command -v nix >/dev/null 2>&1; then
  PKG_TARGET="vidgrab"
  [ "$CLI_ONLY" = "1" ] && PKG_TARGET="vidgrab-cli"
  run_cmd nix --extra-experimental-features "nix-command flakes" build "$ROOT_DIR/packaging/nix#$PKG_TARGET"
  echo "Binaire construit via Nix flake (./result/bin/)."
  exit 0
fi

# Immutable rpm-ostree systems (Fedora Silverblue / Kinoite / Bazzite)
if [ -e /run/ostree-booted ] && command -v rpm-ostree >/dev/null 2>&1; then
  echo "Système immuable OSTree détecté ($OS_NAME)."
  if command -v flatpak >/dev/null 2>&1; then
    echo "Recommandé : flatpak-builder build-dir packaging/flatpak/io.github.vidgrab.VidGrab.yml"
  fi
  if [ "$CLI_ONLY" = "1" ]; then
    as_root rpm-ostree install --idempotent ffmpeg python3
  else
    as_root rpm-ostree install --idempotent ffmpeg wl-clipboard
  fi
  exit 0
fi

# Alpine Linux (apk / musl)
if [ "$OS_ID" = "alpine" ] || command -v apk >/dev/null 2>&1; then
  as_root apk add --no-cache cargo rust musl-dev openssl-dev sqlite-dev pkgconf ca-certificates ffmpeg yt-dlp python3
  run_cmd cargo install --path "$ROOT_DIR/crates/vidgrab-cli" --locked
  exit 0
fi

# Debian family (Debian, Ubuntu, Mint, Pop!_OS, Zorin, Elementary, Kali, Parrot, Tails, MX, RPi OS)
if command -v apt-get >/dev/null 2>&1; then
  PKG_MGR="apt-get"
  command -v apt >/dev/null 2>&1 && PKG_MGR="apt"
  command -v nala >/dev/null 2>&1 && PKG_MGR="nala"
  YES_FLAG=""
  [ "$AUTO_YES" = "1" ] && YES_FLAG="-y"

  as_root apt-get update
  if [ "$CLI_ONLY" = "1" ]; then
    as_root "$PKG_MGR" install ${YES_FLAG:+$YES_FLAG} \
      cargo rustc pkg-config libssl-dev libsqlite3-dev ffmpeg python3 ca-certificates
  else
    as_root "$PKG_MGR" install ${YES_FLAG:+$YES_FLAG} \
      cargo rustc pkg-config libwebkit2gtk-4.1-dev libssl-dev libsqlite3-dev librsvg2-dev \
      libayatana-appindicator3-dev ffmpeg wl-clipboard xclip libnotify-bin ca-certificates
  fi
# Fedora / RHEL / AlmaLinux / Rocky / Nobara
elif command -v dnf >/dev/null 2>&1 || command -v yum >/dev/null 2>&1 || command -v microdnf >/dev/null 2>&1; then
  RPM_MGR="dnf"
  command -v dnf >/dev/null 2>&1 || RPM_MGR="yum"
  command -v yum >/dev/null 2>&1 || command -v dnf >/dev/null 2>&1 || RPM_MGR="microdnf"
  YES_FLAG=""
  [ "$AUTO_YES" = "1" ] && YES_FLAG="-y"
  if [ "$CLI_ONLY" = "1" ]; then
    as_root "$RPM_MGR" install ${YES_FLAG:+$YES_FLAG} \
      cargo rust pkgconf-pkg-config openssl-devel sqlite-devel gcc ffmpeg-free python3
  else
    as_root "$RPM_MGR" install ${YES_FLAG:+$YES_FLAG} \
      cargo rust pkgconf-pkg-config webkit2gtk4.1-devel openssl-devel sqlite-devel \
      librsvg2-devel libayatana-appindicator-gtk3-devel gcc ffmpeg wl-clipboard xclip
  fi
# openSUSE Leap / Tumbleweed (zypper)
elif command -v zypper >/dev/null 2>&1; then
  YES_FLAG=""
  [ "$AUTO_YES" = "1" ] && YES_FLAG="-y"
  if [ "$CLI_ONLY" = "1" ]; then
    as_root zypper --non-interactive install \
      cargo rust pkgconf-pkg-config libopenssl-devel sqlite3-devel gcc ffmpeg yt-dlp python3 || true
  else
    as_root zypper install ${YES_FLAG:+$YES_FLAG} \
      cargo rust pkgconf-pkg-config webkit2gtk3-soup2-devel libopenssl-devel sqlite3-devel \
      librsvg-devel libayatana-appindicator3-1 gcc ffmpeg yt-dlp wl-clipboard xclip
  fi
# Arch / CachyOS / EndeavourOS / Manjaro / Garuda / SteamOS
elif command -v pacman >/dev/null 2>&1; then
  PAC_HELPER="pacman"
  command -v yay >/dev/null 2>&1 && PAC_HELPER="yay"
  command -v paru >/dev/null 2>&1 && PAC_HELPER="paru"
  NC_FLAG=""
  [ "$AUTO_YES" = "1" ] && NC_FLAG="--noconfirm"
  if [ "$CLI_ONLY" = "1" ]; then
    [ "$PAC_HELPER" = "pacman" ] && as_root pacman -Sy --needed ${NC_FLAG:+$NC_FLAG} rust openssl sqlite pkgconf ffmpeg yt-dlp python \
      || run_cmd "$PAC_HELPER" -S --needed ${NC_FLAG:+$NC_FLAG} rust openssl sqlite pkgconf ffmpeg yt-dlp python
  else
    [ "$PAC_HELPER" = "pacman" ] && as_root pacman -Sy --needed ${NC_FLAG:+$NC_FLAG} rust webkit2gtk-4.1 openssl sqlite librsvg libayatana-appindicator pkgconf ffmpeg yt-dlp wl-clipboard xclip \
      || run_cmd "$PAC_HELPER" -S --needed ${NC_FLAG:+$NC_FLAG} rust webkit2gtk-4.1 openssl sqlite librsvg libayatana-appindicator pkgconf ffmpeg yt-dlp wl-clipboard xclip
  fi
# Void Linux (xbps-install)
elif command -v xbps-install >/dev/null 2>&1; then
  YES_FLAG=""
  [ "$AUTO_YES" = "1" ] && YES_FLAG="-y"
  as_root xbps-install -S ${YES_FLAG:+$YES_FLAG} \
    cargo rust pkg-config webkit2gtk41-devel openssl-devel sqlite-devel librsvg-devel \
    libayatana-appindicator-devel ffmpeg yt-dlp
# Gentoo (emerge)
elif command -v emerge >/dev/null 2>&1; then
  as_root emerge --noreplace dev-lang/rust net-libs/webkit-gtk:4.1 dev-libs/openssl dev-db/sqlite \
    gnome-base/librsvg dev-libs/libayatana-appindicator media-video/ffmpeg net-misc/yt-dlp
# Solus (eopkg)
elif command -v eopkg >/dev/null 2>&1; then
  as_root eopkg install -y -c system.devel
  as_root eopkg install -y libwebkit-gtk-devel openssl-devel sqlite3-devel ffmpeg yt-dlp
# Slackware
elif command -v slackpkg >/dev/null 2>&1; then
  echo "Utilisez ./packaging/slackware/vidgrab.SlackBuild après installation de rust, webkit2gtk4.1, ffmpeg et yt-dlp."
else
  echo "Gestionnaire système non reconnu : vérifiez Flatpak, Snap, AppImage ou Cargo."
fi

if [ "$CLI_ONLY" = "1" ]; then
  run_cmd cargo install --path "$ROOT_DIR/crates/vidgrab-cli" --locked
else
  CARGO_FLAGS="--locked"
  if [ -d "$ROOT_DIR/vendor" ] || [ -f "$ROOT_DIR/.cargo/config.toml" ]; then
    CARGO_FLAGS="--offline --locked"
  fi
  run_cmd cargo build --release $CARGO_FLAGS -p vidgrab-tauri -p vidgrab-cli
  run_cmd mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/128x128/apps"
  run_cmd install -Dm755 "$ROOT_DIR/target/release/vidgrab" "$HOME/.local/bin/vidgrab"
  run_cmd install -Dm755 "$ROOT_DIR/target/release/vidgrab-cli" "$HOME/.local/bin/vidgrab-cli"
  run_cmd install -Dm644 "$ROOT_DIR/packaging/vidgrab.desktop" "$HOME/.local/share/applications/vidgrab.desktop"
  run_cmd install -Dm644 "$ROOT_DIR/src-tauri/icons/128x128.png" "$HOME/.local/share/icons/hicolor/128x128/apps/vidgrab.png"
  echo "Installation terminée dans ~/.local/bin (pensez à ajouter ~/.local/bin au PATH)."
fi
