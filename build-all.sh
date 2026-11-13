#!/usr/bin/env bash
set -euo pipefail

# Script de compilation complète pour 4K Free Video Downloader (vidgrab)
# Compile : Frontend, Linux (binaires + .deb/.rpm/.AppImage), Windows x64 (.exe + .zip + setup si makensis),
# et Android APK (si ANDROID_HOME et NDK_HOME sont configurés).
#
# Usage:
#   ./build-all.sh                 # Tout compiler (saute les cibles dont les SDK manquent)
#   ./build-all.sh --skip-tests    # Ignorer cargo test
#   ./build-all.sh --linux-only    # Uniquement Linux
#   ./build-all.sh --windows-only  # Uniquement Windows x64
#   ./build-all.sh --android-only  # Uniquement Android APK

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
DIST_DIR="$ROOT_DIR/dist/$VERSION"
SKIP_TESTS=0
DO_LINUX=1
DO_WINDOWS=1
DO_ANDROID=1

for arg in "$@"; do
  case "$arg" in
    --skip-tests)   SKIP_TESTS=1 ;;
    --linux-only)   DO_LINUX=1; DO_WINDOWS=0; DO_ANDROID=0 ;;
    --windows-only) DO_LINUX=0; DO_WINDOWS=1; DO_ANDROID=0 ;;
    --android-only) DO_LINUX=0; DO_WINDOWS=0; DO_ANDROID=1 ;;
    -h|--help)
      sed -n '3,13p' "$0"
      exit 0
      ;;
    *)
      echo "Option inconnue : $arg" >&2
      exit 1
      ;;
  esac
done

echo "=== 4K Free Video Downloader v${VERSION} — Compilation ==="
mkdir -p "$DIST_DIR"

echo "[1/5] Régénération du frontend (vidgrab-ui)..."
cargo run -p vidgrab-ui --bin build_ui

if [ "$SKIP_TESTS" -eq 0 ]; then
  echo "[2/5] Exécution des tests workspace..."
  cargo test --workspace
else
  echo "[2/5] Tests ignorés (--skip-tests)."
fi

if [ "$DO_LINUX" -eq 1 ]; then
  echo "[3/5] Compilation Linux (binaires + .deb / .rpm / .AppImage)..."
  cargo build --release -p vidgrab-cli
  if cargo tauri build --bundles deb,rpm,appimage; then
    echo "  -> Bundles deb, rpm, appimage générés."
  else
    echo "  -> AppImage a échoué (réseau ou linuxdeploy manquant) ; repli sur deb,rpm..."
    cargo tauri build --bundles deb,rpm
  fi

  mkdir -p "$DIST_DIR/linux"
  cp target/release/vidgrab "$DIST_DIR/linux/vidgrab"
  cp target/release/vidgrab-cli "$DIST_DIR/linux/vidgrab-cli"
  find target/release/bundle/deb -maxdepth 1 -name "*${VERSION}*.deb" -exec cp {} "$DIST_DIR/linux/" \; 2>/dev/null || true
  find target/release/bundle/rpm -maxdepth 1 -name "*${VERSION}*.rpm" -exec cp {} "$DIST_DIR/linux/" \; 2>/dev/null || true
  find target/release/bundle/appimage -maxdepth 1 -name "*${VERSION}*.AppImage" -exec cp {} "$DIST_DIR/linux/" \; 2>/dev/null || true
else
  echo "[3/5] Linux ignoré."
fi

if [ "$DO_WINDOWS" -eq 1 ]; then
  if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "[4/5] Cross-compilation Windows x64 (x86_64-pc-windows-gnu)..."
    rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
    export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
    export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
    cargo build --release --target x86_64-pc-windows-gnu --workspace

    mkdir -p "$DIST_DIR/windows/bundle"
    cp target/x86_64-pc-windows-gnu/release/vidgrab.exe "$DIST_DIR/windows/4k free video dowloader.exe"
    cp target/x86_64-pc-windows-gnu/release/vidgrab-cli.exe "$DIST_DIR/windows/vidgrab-cli.exe"
    if [ -f target/x86_64-pc-windows-gnu/release/WebView2Loader.dll ]; then
      cp target/x86_64-pc-windows-gnu/release/WebView2Loader.dll "$DIST_DIR/windows/WebView2Loader.dll"
      cp "$DIST_DIR/windows/WebView2Loader.dll" "$DIST_DIR/windows/bundle/"
    fi
    cp "$DIST_DIR/windows/4k free video dowloader.exe" "$DIST_DIR/windows/bundle/"
    cp "$DIST_DIR/windows/vidgrab-cli.exe" "$DIST_DIR/windows/bundle/"
    [ -f LICENSE ] && cp LICENSE "$DIST_DIR/windows/bundle/LICENSE"
    if command -v zip >/dev/null 2>&1; then
      (cd "$DIST_DIR/windows/bundle" && zip -qr "../4k free video dowloader_${VERSION}_windows_x64.zip" .)
    fi
    rm -rf "$DIST_DIR/windows/bundle"

    if command -v makensis >/dev/null 2>&1 && [ -x packaging/windows/build-installer.sh ]; then
      ./packaging/windows/build-installer.sh target/x86_64-pc-windows-gnu/release \
        "$DIST_DIR/windows/4k-free-video-dowloader_${VERSION}_setup_x64.exe"
    else
      echo "  -> makensis absent : installeur NSIS ignoré (les .exe et .zip sont prêts)."
    fi
  else
    echo "[4/5] Windows ignoré : x86_64-w64-mingw32-gcc introuvable (installez mingw-w64)."
  fi
else
  echo "[4/5] Windows ignoré."
fi

if [ "$DO_ANDROID" -eq 1 ]; then
  if [ -z "${ANDROID_HOME:-}" ]; then
    for candidate in "${ANDROID_SDK_ROOT:-}" "$HOME/Android/Sdk" "/usr/lib/android-sdk" "/opt/android-sdk"; do
      if [ -n "$candidate" ] && [ -d "$candidate" ]; then
        export ANDROID_HOME="$candidate"
        break
      fi
    done
  fi

  if [ -n "${ANDROID_HOME:-}" ] && { [ -z "${NDK_HOME:-}" ] || [ ! -d "${NDK_HOME}/toolchains/llvm/prebuilt" ]; }; then
    export NDK_HOME=""
    if [ -d "$ANDROID_HOME/ndk" ]; then
      for ndk_dir in $(ls -1d "$ANDROID_HOME/ndk"/*/ 2>/dev/null | sort -rV); do
        ndk_dir="${ndk_dir%/}"
        if [ -d "$ndk_dir/toolchains/llvm/prebuilt" ]; then
          export NDK_HOME="$ndk_dir"
          break
        fi
      done
    fi
    if [ -z "$NDK_HOME" ] && [ -d "$ANDROID_HOME/ndk-bundle/toolchains/llvm/prebuilt" ]; then
      export NDK_HOME="$ANDROID_HOME/ndk-bundle"
    fi
  fi

  if [ -n "${ANDROID_HOME:-}" ] && [ -n "${NDK_HOME:-}" ] && [ -d "${NDK_HOME}/toolchains/llvm/prebuilt" ]; then
    echo "[5/5] Compilation Android APK (aarch64) avec NDK=$NDK_HOME..."
    rustup target add aarch64-linux-android >/dev/null 2>&1 || true
    if [ ! -f "$HOME/.android/debug.keystore" ]; then
      mkdir -p "$HOME/.android"
      keytool -genkey -v -keystore "$HOME/.android/debug.keystore" \
        -storepass android -alias androiddebugkey -keypass android \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Android Debug,O=Android,C=US"
    fi
    cargo tauri android build --apk --target aarch64
    mkdir -p "$DIST_DIR/android"
    APK_SRC="src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk"
    if [ -f "$APK_SRC" ]; then
      cp "$APK_SRC" "$DIST_DIR/android/4k-free-video-dowloader_${VERSION}_android_arm64.apk"
    fi
  else
    echo "[5/5] Android ignoré : Android SDK ou NDK valide introuvable."
    echo "      ANDROID_HOME='${ANDROID_HOME:-}' | NDK_HOME='${NDK_HOME:-}'"
    echo "      Installez le NDK via : sdkmanager \"ndk;27.2.12479018\" \"platforms;android-36\" \"build-tools;36.0.0\""
  fi
else
  echo "[5/5] Android ignoré."
fi

echo "=== Génération des empreintes SHA256 ==="
(
  cd "$DIST_DIR"
  find . -maxdepth 2 -type f ! -name 'SHA256SUMS.txt' -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS.txt
)

echo "=== Terminé ! Artefacts disponibles dans : $DIST_DIR ==="
find "$DIST_DIR" -maxdepth 2 -type f | sort
