Name:           vidgrab
Version:        0.3.0
Release:        1%{?dist}
Summary:        Open-source 4K/8K video & audio downloader (yt-dlp + ffmpeg) in Rust
License:        GPL-3.0-or-later
URL:            https://github.com/Fazerdino1/4k-free-video-dowloader-
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rustc
BuildRequires:  pkgconf-pkg-config
%if 0%{?suse_version}
BuildRequires:  webkit2gtk3-soup2-devel
%else
BuildRequires:  webkit2gtk4.1-devel
BuildRequires:  openssl-devel
BuildRequires:  sqlite-devel
BuildRequires:  librsvg2-devel
BuildRequires:  libayatana-appindicator-gtk3-devel
%endif

%if 0%{?suse_version}
Requires:       libwebkit2gtk-4_1-0
Recommends:     libayatana-appindicator3-1
Recommends:     ffmpeg
Recommends:     wl-clipboard
%else
Requires:       webkit2gtk4.1
Recommends:     (libayatana-appindicator-gtk3 or libappindicator-gtk3)
Recommends:     (ffmpeg or ffmpeg-free)
Recommends:     wl-clipboard
%endif

%description
4k free video dowloader (vidgrab) is a lightweight open-source video, playlist, and audio
downloader written 100% in Rust (Tauri 2 + WebKitGTK) powered by yt-dlp and ffmpeg.
Includes both the desktop GUI (`vidgrab`) and the zero-GTK CLI (`vidgrab-cli`).
Compatible with Fedora, RHEL, AlmaLinux, Rocky Linux, Nobara, Bazzite (rpm-ostree),
openSUSE Leap/Tumbleweed (zypper).

%package cli
Summary:        Headless CLI for 4k free video dowloader (servers & containers)
Recommends:     (ffmpeg or ffmpeg-free)
Recommends:     yt-dlp

%description cli
Zero-GTK command-line binary (`vidgrab-cli`) for headless RHEL/Alma/Rocky/Fedora/openSUSE
servers and minimal containers.

%prep
%autosetup

%build
if [ -d vendor ] || [ -f .cargo/config.toml ]; then
  cargo build --release --offline -p vidgrab-tauri -p vidgrab-cli
else
  cargo build --release --locked -p vidgrab-tauri -p vidgrab-cli
fi

%install
install -Dm755 target/release/vidgrab %{buildroot}%{_bindir}/vidgrab
install -Dm755 target/release/vidgrab-cli %{buildroot}%{_bindir}/vidgrab-cli
install -Dm644 LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
install -Dm644 packaging/vidgrab.desktop %{buildroot}%{_datadir}/applications/vidgrab.desktop
install -Dm644 packaging/metainfo/io.github.vidgrab.VidGrab.metainfo.xml %{buildroot}%{_datadir}/metainfo/io.github.vidgrab.VidGrab.metainfo.xml
install -Dm644 src-tauri/icons/128x128.png %{buildroot}%{_datadir}/icons/hicolor/128x128/apps/vidgrab.png
install -Dm644 src-tauri/icons/32x32.png %{buildroot}%{_datadir}/icons/hicolor/32x32/apps/vidgrab.png
install -Dm644 src-tauri/icons/icon.png %{buildroot}%{_datadir}/icons/hicolor/512x512/apps/vidgrab.png

%files
%license LICENSE
%{_bindir}/vidgrab
%{_datadir}/applications/vidgrab.desktop
%{_datadir}/metainfo/io.github.vidgrab.VidGrab.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/vidgrab.png

%files cli
%license LICENSE
%{_bindir}/vidgrab-cli

%changelog
* Thu Nov 12 2026 Fazerdino1 <187407683+Fazerdino1@users.noreply.github.com> - 0.3.0-1
- Harmonize GPL-3.0-or-later license, harden Tauri WebView CSP, and support Windows .exe
