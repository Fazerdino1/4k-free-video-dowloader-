# 4k free video dowloader (`vidgrab` & `vidgrab-cli`)

Dépôt officiel : [Fazerdino1/4k-free-video-dowloader-](https://github.com/Fazerdino1/4k-free-video-dowloader-) • Licence : **[GPL-3.0-or-later](LICENSE)**

Application **open source**, écrite **100 % en Rust** (Tauri 2 + WebView2 / WebKitGTK + CLI headless), propulsée par **yt-dlp** et **ffmpeg**, compatible avec **Windows 10/11 (x64)** et l'ensemble des distributions Linux **Desktop, Serveur, ARM, Gaming, Cybersécurité et Conteneurs**.

## Téléchargements précompilés (`v0.2.0`)

Tous les binaires et paquets précompilés sont distribués exclusivement via la [**Release GitHub `v0.2.0`**](https://github.com/Fazerdino1/4k-free-video-dowloader-/releases/tag/v0.2.0) (accompagnés du fichier `SHA256SUMS.txt` ; le dépôt Git ne contient que les sources) :

| Plateforme / Format | Usage cible | Fichier |
|---|---|---|
| **Windows GUI (x64)** | Application Desktop Windows 10/11 *(placer `WebView2Loader.dll` dans le même dossier)* | `4k free video dowloader.exe` + `WebView2Loader.dll` |
| **Windows Archive complète** | Archive portable prête à l'emploi (`4k free video dowloader.exe` + `vidgrab-cli.exe` + `WebView2Loader.dll` + `LICENSE`) | `4k free video dowloader_0.2.0_windows_x64.zip` |
| **Windows CLI (x64)** | Ligne de commande headless Windows | `vidgrab-cli.exe` |
| **AppImage** (universel Linux) | Toute distribution Linux (`chmod +x`) | `4k free video dowloader_0.2.0_amd64.AppImage` |
| **RPM** | Fedora, Nobara, RHEL, AlmaLinux, Rocky, openSUSE | `4k free video dowloader-0.2.0-1.x86_64.rpm` |
| **DEB** | Ubuntu, Linux Mint, Debian, Pop!_OS, Kali, Parrot, Zorin | `4k free video dowloader_0.2.0_amd64.deb` |
| **GUI Linux (x86_64)** | Exécutable Desktop Rust/WebKitGTK | `vidgrab` |
| **CLI Headless (x86_64)** | Serveurs, Raspberry Pi / SSH / conteneurs sans GTK | `vidgrab-cli` |

- **Gratuit et sans limite artificielle** : aucun verrou payant, téléchargements simultanés illimités, abonnements automatiques, pistes doublées, découpe temporelle (`--download-sections` / FFmpeg), miniatures et sous-titres complets.
- **Universalité Linux native** :
  - Détection automatique de l'architecture (`x86_64`, `aarch64`, `armv7l`), de la `libc` (`glibc` / Alpine `musl`), du gestionnaire d'init (`systemd`, `OpenRC`, `runit`, `s6`, `SysVinit`), de l'affichage (`Wayland`, `XWayland`, `X11`, `Headless`), des pilotes NVIDIA, et des environnements **Flatpak / Snap / AppImage / rpm-ostree / NixOS / Qubes OS / Tails Tor**.
  - Binaire **`vidgrab-cli`** sans aucune dépendance GTK/WebKitGTK pour serveurs (**Ubuntu Server, Debian, RHEL, Alma, Rocky, Alpine, Raspberry Pi OS Lite, Docker**).
- **Respect du DRM** : détecte les flux protégés par Widevine/PlayReady (Netflix, Disney+, Prime Video, Spotify, etc.) et affiche un message explicite sans tenter de contournement.

---

## Installation rapide (multi-distributions)

Un script universel détecte automatiquement votre distribution, son architecture et son gestionnaire de paquets (`apt`, `nala`, `dnf`, `rpm-ostree`, `zypper`, `pacman`, `yay`, `paru`, `apk`, `emerge`, `xbps-install`, `nix`, `eopkg`, `slackpkg`) :

```bash
# Aperçu sans modification (--dry-run)
./packaging/install-linux.sh --dry-run

# Installation Desktop complète
./packaging/install-linux.sh

# Mode serveur / headless (binaire vidgrab-cli uniquement)
./packaging/install-linux.sh --cli-only
```

Consultez la matrice complète dans [docs/LINUX_COMPATIBILITY.md](docs/LINUX_COMPATIBILITY.md) (Ubuntu 26.04, Mint 22.3, Fedora 44, Pop!_OS 24.04, Zorin, Elementary, Debian 13 Trixie, RHEL/Alma/Rocky, openSUSE Leap/Tumbleweed, Alpine, Arch/CachyOS/EndeavourOS/Manjaro/Garuda, Gentoo, Void, NixOS, Guix, Solus, Slackware, Bazzite, SteamOS, Nobara, Kali, Parrot, Tails, Qubes, Raspberry Pi OS, MX Linux + Flatpak, Snap, AppImage).

---

## Utilisation en ligne de commande (Desktop & Serveur Headless)

Les commandes CLI fonctionnent autant avec `vidgrab-cli` (zéro GTK) qu'avec `vidgrab` avant l'initialisation graphique :

```bash
# Vérifier la détection Linux, architecture, libc, proxy Tor, yt-dlp et ffmpeg
vidgrab-cli diag

# Installer ou mettre à jour yt-dlp selon votre architecture / libc
vidgrab-cli update

# Télécharger une vidéo 1080p ou extraire en MP3 / FLAC
vidgrab-cli download "https://www.youtube.com/watch?v=..." -q 1080 -o ~/Videos
vidgrab-cli download "https://www.youtube.com/watch?v=..." --audio flac --start 00:30 --end 02:00

# Boîte à outils FFmpeg locale (remux / mp3 / flac / h264 / trim)
vidgrab-cli ffmpeg trim ./video.mkv --start 01:00 --end 02:30
```

---

## Architecture du Workspace Cargo

```text
vidgrab/
├── Cargo.toml                 # [workspace] Rust
├── crates/
│   ├── vidgrab-core/          # Moteur yt-dlp, platform.rs (25+ distros), cli.rs, SQLite, queue, media_tools
│   ├── vidgrab-cli/           # Binaire CLI pur Rust (zéro GTK, serveurs / Alpine musl / ARM)
│   ├── vidgrab-shared/        # Types serde partagés backend / CLI / frontend
│   └── vidgrab-ui/            # Interface HTML/CSS/JS adaptative KDE / GNOME / COSMIC / Xfce
├── src-tauri/                 # Binaire Desktop Tauri 2 (GUI + CLI + tray tolérant + RPM/DEB)
├── docs/LINUX_COMPATIBILITY.md
└── packaging/
    ├── install-linux.sh       # Installateur universel multi-distros (--dry-run / --cli-only)
    ├── vidgrab.desktop        # Entrée XDG universelle
    ├── metainfo/              # AppStream metainfo.xml
    ├── debian/                # Debian / Ubuntu / Mint / Pop / Kali / RPi OS (.deb)
    ├── rpm/                   # Fedora / RHEL / Alma / Rocky / Nobara / openSUSE (.rpm)
    ├── arch/                  # PKGBUILD + .SRCINFO (Arch / CachyOS / Manjaro / Garuda)
    ├── alpine/                # APKBUILD (musl + OpenRC)
    ├── gentoo/                # Ebuild Portage (USE="gui wayland X")
    ├── void/                  # Template xbps-src
    ├── nix/                   # Flake Nix (x86_64-linux & aarch64-linux)
    ├── guix/                  # Recette GNU Guix
    ├── solus/                 # package.yml (eopkg)
    ├── slackware/             # vidgrab.SlackBuild + slack-desc
    ├── flatpak/               # io.github.vidgrab.VidGrab.yml
    ├── snap/                  # snapcraft.yaml (core24)
    ├── appimage/              # build-appimage.sh
    └── docker/                # Dockerfile multi-stage Alpine musl (non-root)
```

---

## Compilation & génération des paquets depuis les sources

### Vérification qualité & tests

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

### Construction des paquets `.deb`, `.rpm` et `.AppImage` (Tauri)

```bash
cargo tauri build --bundles deb,rpm,appimage
```

### Cross-compilation Windows x64 (depuis Linux avec MinGW-w64)

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
cargo build --release --locked --target x86_64-pc-windows-gnu --workspace
```

### Image Docker serveur headless (Alpine `musl`, non-root)

```bash
docker build -f packaging/docker/Dockerfile -t vidgrab-cli:latest .
docker run --rm -v "$PWD/downloads:/downloads" vidgrab-cli:latest diag
```

## Sécurité & Licence

- **Licence** : GNU General Public License v3.0 or later ([`LICENSE`](LICENSE)).
- **Durcissement WebView** : politique CSP restrictive dans [`src-tauri/tauri.conf.json`](src-tauri/tauri.conf.json), permissions Tauri limitées à la fenêtre principale dans [`src-tauri/capabilities/default.json`](src-tauri/capabilities/default.json), et filtrage des URLs d'images/miniatures dans l'interface.
- **CI/CD GitHub Actions** : [`ci.yml`](.github/workflows/ci.yml) (`rustfmt`, `clippy` Linux & Windows GNU, tests d'intégration, compilation croisée) et [`release.yml`](.github/workflows/release.yml) pour la publication automatisée sur tags `v*`.
