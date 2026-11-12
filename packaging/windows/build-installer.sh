#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
WIN_RELEASE_DIR="${1:-$ROOT_DIR/target/x86_64-pc-windows-gnu/release}"
OUT_SETUP="${2:-$ROOT_DIR/dist/windows/4k-free-video-dowloader_0.3.0_setup_x64.exe}"

mkdir -p "$(dirname "$OUT_SETUP")"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "==> [1/4] Compilation des ressources Windows (icône + manifest UAC)..."
(cd "$SCRIPT_DIR" && x86_64-w64-mingw32-windres installer.rc -O coff -o "$WORK_DIR/installer_res.o")
(cd "$SCRIPT_DIR" && x86_64-w64-mingw32-windres uninstaller.rc -O coff -o "$WORK_DIR/uninstaller_res.o")

echo "==> [2/4] Compilation du désinstallateur permanent (uninstall.exe)..."
x86_64-w64-mingw32-gcc -O2 -Os -s -Wall -Wextra -municode -mwindows \
  "$SCRIPT_DIR/uninstaller.c" "$WORK_DIR/uninstaller_res.o" \
  -lshell32 -lole32 -ladvapi32 -luser32 \
  -o "$WORK_DIR/uninstall.exe"

echo "==> [3/4] Compilation du stub d'installation graphique (installer_stub.exe)..."
x86_64-w64-mingw32-gcc -O2 -Os -s -Wall -Wextra -municode -mwindows \
  "$SCRIPT_DIR/installer.c" "$WORK_DIR/installer_res.o" \
  -lshell32 -lole32 -luuid -lcomctl32 -lgdi32 -ladvapi32 -luser32 \
  -o "$WORK_DIR/installer_stub.exe"

echo "==> [4/4] Compression LZ4 et assemblage de l'installateur permanent..."
gcc -O3 -Wall -Wextra "$SCRIPT_DIR/pack_payload.c" -o "$WORK_DIR/pack_payload"

"$WORK_DIR/pack_payload" \
  "$WORK_DIR/installer_stub.exe" \
  "$OUT_SETUP" \
  "4k free video dowloader.exe=$WIN_RELEASE_DIR/vidgrab.exe" \
  "WebView2Loader.dll=$WIN_RELEASE_DIR/WebView2Loader.dll" \
  "vidgrab-cli.exe=$WIN_RELEASE_DIR/vidgrab-cli.exe" \
  "uninstall.exe=$WORK_DIR/uninstall.exe" \
  "LICENSE=$ROOT_DIR/LICENSE"

ls -lh "$OUT_SETUP"
echo "==> Installateur Windows créé : $OUT_SETUP"
