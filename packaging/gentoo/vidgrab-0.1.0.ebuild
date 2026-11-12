# Copyright 2026 VidGrab Contributors
# Distributed under the terms of the MIT or Apache-2.0 License

EAPI=8

inherit cargo xdg

DESCRIPTION="Open-source 4K/8K video & audio downloader (yt-dlp + ffmpeg) in Rust"
HOMEPAGE="https://github.com/vidgrab/4kfreedowloader"

LICENSE="MIT Apache-2.0"
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
	cargo build --release --offline "${pkgs[@]}" || die
}

src_install() {
	dobin target/release/vidgrab-cli
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
