# Matrice complète de compatibilité Linux — **4k free video dowloader (`vidgrab` & `vidgrab-cli`)**

Cette documentation détaille la prise en charge multi-distributions, multi-architectures (`x86_64`, `aarch64`, `armv7l`), multi-libc (`glibc` / `musl`), multi-init (`systemd`, `OpenRC`, `runit`, `s6`, `sysvinit`) et multi-formats.

---

## 1. Architecture binaire adaptée aux environnements

| Binaire | Dépendances graphiques | Cible principale |
| :--- | :--- | :--- |
| `vidgrab` | GTK3 + WebKitGTK 4.1 | Postes de travail (KDE Plasma, GNOME, COSMIC, Cinnamon, Xfce, MATE, Pantheon, LXQt, Hyprland/Sway) + CLI intégrée (`vidgrab diag/analyze/download/ffmpeg`) |
| `vidgrab-cli` | **Aucune (0 GTK/WebKit)** | Serveurs cloud (`Ubuntu Server`, `Debian`, `RHEL`, `AlmaLinux`, `Rocky`), `Alpine Linux` (`musl`), `Raspberry Pi OS Lite`, conteneurs Docker/K8s |

---

## 2. Adaptations dynamiques au runtime (`vidgrab_core::platform`)

1. **Détection automatique d'architecture & de `libc` pour `yt-dlp`** :
   - `x86_64` (`glibc`) → `yt-dlp_linux`
   - `aarch64` / ARM64 (`glibc`, ex. Raspberry Pi OS 64 bits) → `yt-dlp_linux_aarch64`
   - `armv7l` (`glibc`, Raspberry Pi 32 bits) → `yt-dlp_linux_armv7l`
   - `Alpine Linux` (`musl`) & `NixOS` → binaire système `yt-dlp` ou zipapp portable Python 3 `yt-dlp`
2. **Wayland / XWayland / X11 / Headless** :
   - Préférence `GDK_BACKEND=wayland,x11` sous session Wayland pure, `x11,wayland` sous XWayland/X11.
   - Si ni `WAYLAND_DISPLAY` ni `DISPLAY` n'est présent (`headless`), `vidgrab` bascule sur un message d'aide CLI propre sans crash GTK.
3. **NVIDIA, COSMIC, Qubes OS, Tails & Machines Virtuelles** :
   - `WEBKIT_DISABLE_DMABUF_RENDERER=1` appliqué automatiquement si non défini.
   - `WEBKIT_DISABLE_COMPOSITING_MODE=1` activé dynamiquement uniquement lorsque nécessaire (pilote propriétaire NVIDIA `/sys/module/nvidia`, Qubes OS `/run/qubes`, ou absence de nœud DRM `/dev/dri/renderD*`).
4. **System Tray sécurisé** :
   - Si le bureau n'expose pas d'icône StatusNotifier/AppIndicator (GNOME Vanilla sans extension, COSMIC, Elementary Pantheon, Tails), fermer la fenêtre principale quitte proprement l'application au lieu de la masquer définitivement.
5. **Extinction post-téléchargement multi-init** :
   - Chaîne automatique : `systemctl poweroff` (systemd) → `loginctl poweroff` (elogind) → `poweroff` (OpenRC / Alpine) → `shutdown -h now` (runit / SysVinit).
6. **Anonymat Tails OS / Qubes OS / Parrot OS** :
   - Détection locale des ports Tor SOCKS5 (`127.0.0.1:9050` / `9150`) et préconfiguration automatique `socks5h://127.0.0.1:9050` sous Tails lors de l'initialisation.

---

## 3. Matrice des distributions & gestionnaires de paquets

### Poste de travail & grand public
| Distribution | Bureau / Particularité | Gestionnaire(s) | Recette / Paquet |
| :--- | :--- | :--- | :--- |
| **Ubuntu 26.04 LTS** (+ Kubuntu, Xubuntu, Lubuntu) | GNOME / KDE / Xfce / LXQt + Snap/APT | `apt`, `apt-get`, `nala`, `dpkg`, `aptitude`, `snap` | [debian/control](../packaging/debian/control) / `.deb` |
| **Linux Mint 22.3 & LMDE** | Cinnamon sans Snap par défaut | `apt`, `nala`, `dpkg`, `flatpak` | `.deb` / Flatpak |
| **Fedora 44** (+ Silverblue / Kinoite) | Wayland récent, immuable (`rpm-ostree`) | `dnf`, `rpm`, `rpm-ostree`, `flatpak` | [rpm/vidgrab.spec](../packaging/rpm/vidgrab.spec) / `.rpm` |
| **Pop!_OS 24.04 LTS** | COSMIC (Rust) + NVIDIA natif | `apt`, `flatpak` | `.deb` / Flatpak |
| **Zorin OS** | Interface Bureau + Flatpak | `apt`, `flatpak` | `.deb` / Flatpak |
| **Elementary OS** | Pantheon (sans tray AppIndicator par défaut) | `apt`, `flatpak` | `.deb` / Flatpak |

### Serveurs, cloud & entreprise
| Distribution | Cible & spécificité | Gestionnaire(s) | Recette / Paquet |
| :--- | :--- | :--- | :--- |
| **Debian 13 « Trixie »** | Stable / Serveurs auto-hébergés | `dpkg`, `apt`, `apt-get`, `aptitude`, `nala` | [debian/control](../packaging/debian/control) |
| **Ubuntu Server** | Cloud (AWS/Azure/GCP), Kubernetes | `apt`, `snap` | `vidgrab-cli` (`.deb` / Snap CLI) |
| **RHEL / AlmaLinux / Rocky Linux** | Entreprise, conteneurs minimalistes | `rpm`, `dnf`, `yum`, `microdnf` | [rpm/vidgrab.spec](../packaging/rpm/vidgrab.spec) (`vidgrab-cli`) |
| **openSUSE Leap 16.0 & Tumbleweed** | RPM avec moteur `libsolv` | `zypper`, `rpm` | [rpm/vidgrab.spec](../packaging/rpm/vidgrab.spec) / `.rpm` |
| **Alpine Linux** | `musl` libc + BusyBox + OpenRC | `apk` | [alpine/APKBUILD](../packaging/alpine/APKBUILD) & [Dockerfile](../packaging/docker/Dockerfile) |

### Utilisateurs avancés & « Rolling Release »
| Distribution | Spécificité | Gestionnaire(s) | Recette |
| :--- | :--- | :--- | :--- |
| **Arch Linux** | Rolling release, AUR | `pacman`, `yay`, `paru` | [arch/PKGBUILD](../packaging/arch/PKGBUILD) |
| **CachyOS** | Paquets x86-64-v3/v4, noyau optimisé | `pacman`, `paru`, `yay` | [arch/PKGBUILD](../packaging/arch/PKGBUILD) |
| **EndeavourOS / Manjaro / Garuda** | Basées sur Arch (AUR helpers / Btrfs) | `pacman`, `yay`, `paru` | [arch/PKGBUILD](../packaging/arch/PKGBUILD) |
| **Gentoo Linux** | Compilation locale Portage (`USE="-gui"` serveur) | `emerge` (Portage) | [gentoo/vidgrab-0.1.0.ebuild](../packaging/gentoo/vidgrab-0.1.0.ebuild) |
| **Void Linux / Solus / Slackware** | `runit` (`xbps`), `eopkg`, `SlackBuild` | `xbps-install`, `eopkg`, `slackpkg` | [void/template](../packaging/void/template), [solus/package.yml](../packaging/solus/package.yml), [slackware/](../packaging/slackware/vidgrab.SlackBuild) |
| **NixOS / GNU Guix** | Déclaratif & reproductible | `nix`, `guix` | [nix/flake.nix](../packaging/nix/flake.nix), [guix/guix.scm](../packaging/guix/guix.scm) |

### Gaming, Cybersécurité & ARM
| Distribution | Spécificité | Gestionnaire(s) | Notes intégrées |
| :--- | :--- | :--- | :--- |
| **Bazzite** | Fedora Atomic immuable gaming | `rpm-ostree`, `flatpak` | Flatpak ou layering `rpm-ostree` |
| **SteamOS** | Arch immuable / Gamescope (Steam Deck) | `pacman`, `flatpak` | Mode Bureau Flatpak ou AppImage |
| **Nobara Linux 44** | Noyau & codecs multimédia | `dnf`, `rpm` | Paquet `.rpm` natif testé |
| **Kali Linux / Parrot OS** | Debian Testing / MATE + Anonsurf Tor | `apt`, `nala` | `.deb` + détection proxy Tor |
| **Tails OS** | Live amnésique route Tor intégrale | `apt` / AppImage persistant | Auto-configuration proxy `socks5h://127.0.0.1:9050` |
| **Qubes OS** | Cloisonnement AppVM / Xen | `apt` / `dnf` dans TemplateVM | Pas d'accélération GPU DRM → software compositing WebKit sûr |
| **Raspberry Pi OS (Desktop & Lite)** | ARM64 (`aarch64`) / ARM32 (`armv7l`) | `apt`, `dpkg` | `yt-dlp_linux_aarch64` / `armv7l` auto-sélectionné ; CLI sur Lite |
| **MX Linux** | Debian Stable + Xfce, SysVinit/systemd-shim | `apt`, `dpkg` | `.deb` + fallback `poweroff` SysVinit |

---

## 4. Formats universels Linux

- **Flatpak** : `flatpak-builder build-dir packaging/flatpak/io.github.vidgrab.VidGrab.yml` (voir [Flatpak manifest](../packaging/flatpak/io.github.vidgrab.VidGrab.yml)).
- **Snap** : `snapcraft pack` à l'aide de [snapcraft.yaml](../packaging/snap/snapcraft.yaml) (`core24`, commandes `vidgrab` et `vidgrab.cli`).
- **AppImage** : `./packaging/appimage/build-appimage.sh` produit un `AppDir` ou `.AppImage` autonome (`x86_64` / `aarch64`).
- **Docker (Alpine `musl` non-root)** : `docker build -f packaging/docker/Dockerfile -t vidgrab-cli .`.
- **Script d'installation multi-distros** : `./packaging/install-linux.sh --dry-run` (ou `--cli-only` sur serveur headless).
