#!/usr/bin/env bash
# Builds a self-contained portable AppImage for x86_64 or aarch64 Linux.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ARCH="${ARCH:-$(uname -m)}"
APPDIR="$ROOT_DIR/target/AppDir"

cd "$ROOT_DIR"
cargo build --release --offline -p vidgrab-tauri -p vidgrab-cli

rm -rf "$APPDIR"
mkdir -p \
  "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo" \
  "$APPDIR/usr/share/icons/hicolor/128x128/apps" \
  "$APPDIR/usr/share/icons/hicolor/512x512/apps"

install -Dm755 target/release/vidgrab "$APPDIR/usr/bin/vidgrab"
install -Dm755 target/release/vidgrab-cli "$APPDIR/usr/bin/vidgrab-cli"
install -Dm644 packaging/vidgrab.desktop "$APPDIR/usr/share/applications/vidgrab.desktop"
install -Dm644 packaging/vidgrab.desktop "$APPDIR/vidgrab.desktop"
install -Dm644 packaging/metainfo/io.github.vidgrab.VidGrab.metainfo.xml \
  "$APPDIR/usr/share/metainfo/io.github.vidgrab.VidGrab.metainfo.xml"
install -Dm644 src-tauri/icons/128x128.png "$APPDIR/usr/share/icons/hicolor/128x128/apps/vidgrab.png"
install -Dm644 src-tauri/icons/icon.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/vidgrab.png"
cp src-tauri/icons/icon.png "$APPDIR/vidgrab.png"

cat > "$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export APPDIR="$HERE"
if [ "$(basename "$0")" = "vidgrab-cli" ]; then
  exec "$HERE/usr/bin/vidgrab-cli" "$@"
fi
exec "$HERE/usr/bin/vidgrab" "$@"
EOF
chmod +x "$APPDIR/AppRun"

if command -v appimagetool >/dev/null 2>&1; then
  OUT="$ROOT_DIR/target/4k-free-video-dowloader-${ARCH}.AppImage"
  ARCH="$ARCH" appimagetool "$APPDIR" "$OUT"
  echo "AppImage generated: $OUT"
else
  echo "AppDir prepared cleanly at: $APPDIR"
  echo "Install appimagetool (or linuxdeploy) to pack $APPDIR into .AppImage"
fi
