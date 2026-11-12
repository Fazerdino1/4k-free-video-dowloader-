# Copyright 2026 Fazerdino1
# Distributed under the terms of the GNU General Public License v3 or later

EAPI=8

inherit cargo xdg

DESCRIPTION="Open-source 4K/8K video & audio downloader (yt-dlp + ffmpeg) in Rust"
HOMEPAGE="https://github.com/Fazerdino1/4k-free-video-dowloader-"

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
IUSE="+gui wayland X"

RDEPEND="
	dev-db/sqlite:3
	dev-libs/openssl:=
	media-video/ffmpeg
	net-misc/yt-dlp
	gui? (
		net-libs/webkit-gtk:4.1
		x11-libs/gtk+:3[wayland?,X?]
		dev-libs/libayatana-appindicator
		wayland? ( gui-apps/wl-clipboard )
		X? ( x11-misc/xclip )
	)
"
DEPEND="${RDEPEND}"
BDEPEND="virtual/pkgconfig"

src_compile() {
	local pkgs=( -p vidgrab-cli )
	use gui && pkgs+=( -p vidgrab-tauri )
	if [[ -d vendor ]] || [[ -d "${ECARGO_VENDOR:-/nonexistent}" ]]; then
		cargo build --release --offline "${pkgs[@]}" || die
	else
		cargo build --release --locked "${pkgs[@]}" || die
	fi
}

src_install() {
	dobin target/release/vidgrab-cli
	dodoc LICENSE README.md
	if use gui; then
		dobin target/release/vidgrab
		insinto /usr/share/applications
		doins packaging/vidgrab.desktop
		insinto /usr/share/metainfo
		doins packaging/metainfo/io.github.vidgrab.VidGrab.metainfo.xml
		newicon -s 128 src-tauri/icons/128x128.png vidgrab.png
		newicon -s 32 src-tauri/icons/32x32.png vidgrab.png
	fi
}
