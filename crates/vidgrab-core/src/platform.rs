//! Cross-distribution Linux environment, architecture, libc, init system,
//! sandbox/immutable OS, display server, and Tor proxy detection.

use crate::config::AppPaths;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;
use vidgrab_shared::{LinuxPlatformInfo, Settings};

/// Parses `/etc/os-release` key-value pairs into `(id, pretty_name, id_like)`.
#[must_use]
pub fn parse_os_release(content: &str) -> (String, String, Vec<String>) {
    let mut id = String::new();
    let mut name = String::new();
    let mut pretty = String::new();
    let mut id_like = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, raw_v)) = trimmed.split_once('=') else {
            continue;
        };
        let v = raw_v
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match k.trim() {
            "ID" => id = v.to_lowercase(),
            "NAME" => name = v,
            "PRETTY_NAME" => pretty = v,
            "ID_LIKE" => {
                id_like = v
                    .split_whitespace()
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }

    let resolved_name = if !pretty.is_empty() {
        pretty
    } else if !name.is_empty() {
        name
    } else if !id.is_empty() {
        id.clone()
    } else {
        "Linux".to_string()
    };
    let resolved_id = if id.is_empty() {
        "linux".to_string()
    } else {
        id
    };

    (resolved_id, resolved_name, id_like)
}

/// Detects the C library (`"musl"` on Alpine/musl systems or `"glibc"`).
#[must_use]
pub fn detect_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        return "musl";
    }
    let has_musl_loader = [
        "/lib/ld-musl-x86_64.so.1",
        "/lib/ld-musl-aarch64.so.1",
        "/lib/ld-musl-armhf.so.1",
        "/lib/ld-musl-riscv64.so.1",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    let has_glibc_loader = [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/lib/x86_64-linux-gnu/libc.so.6",
        "/lib/aarch64-linux-gnu/libc.so.6",
    ]
    .iter()
    .any(|p| Path::new(p).exists());

    if has_musl_loader && !has_glibc_loader {
        "musl"
    } else {
        "glibc"
    }
}

/// Detects the active Linux init/service manager (`systemd`, `openrc`, `runit`, `s6`, `sysvinit`).
#[must_use]
pub fn detect_init_system() -> &'static str {
    if Path::new("/run/systemd/system").exists() {
        "systemd"
    } else if Path::new("/run/openrc").exists() {
        "openrc"
    } else if Path::new("/run/runit").exists() || Path::new("/etc/runit").exists() {
        "runit"
    } else if Path::new("/run/s6").exists() || Path::new("/etc/s6").exists() {
        "s6"
    } else if Path::new("/sbin/init").exists() {
        "sysvinit"
    } else {
        "unknown"
    }
}

/// Detects whether the app runs inside Flatpak, Snap, AppImage, NixOS, OSTree, SteamOS, or a container.
#[must_use]
pub fn detect_sandbox_runtime(distro_id: &str) -> (&'static str, bool) {
    if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
        return ("flatpak", true);
    }
    if std::env::var_os("SNAP").is_some() {
        return ("snap", true);
    }
    if std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some() {
        return ("appimage", false);
    }
    if Path::new("/etc/NIXOS").exists() {
        return ("nixos", true);
    }
    if Path::new("/run/ostree-booted").exists() {
        return ("rpm-ostree", true);
    }
    if distro_id == "steamos" || Path::new("/etc/steamos-release").exists() {
        return ("steamos", true);
    }
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return ("container", false);
    }
    ("native", false)
}

/// Detects the active display backend (`wayland`, `wayland+xwayland`, `x11`, or `headless`).
#[must_use]
pub fn detect_display_backend() -> &'static str {
    let wayland = std::env::var_os("WAYLAND_DISPLAY")
        .filter(|v| !v.is_empty())
        .is_some();
    let x11 = std::env::var_os("DISPLAY")
        .filter(|v| !v.is_empty())
        .is_some();
    match (wayland, x11) {
        (true, true) => "wayland+xwayland",
        (true, false) => "wayland",
        (false, true) => "x11",
        (false, false) => "headless",
    }
}

/// Returns `true` when NVIDIA kernel modules are loaded cleanly in `/sys` or `/proc`.
#[must_use]
pub fn detect_nvidia() -> bool {
    Path::new("/sys/module/nvidia").exists() || Path::new("/proc/driver/nvidia/version").exists()
}

/// Returns `true` when running inside a Qubes OS AppVM/TemplateVM.
#[must_use]
pub fn detect_qubes() -> bool {
    std::env::var_os("QUBES_ENV").is_some()
        || Path::new("/run/qubes").exists()
        || Path::new("/var/run/qubes").exists()
}

/// Returns `true` when running on Tails live amnesic OS.
#[must_use]
pub fn detect_tails(distro_id: &str) -> bool {
    distro_id == "tails"
        || Path::new("/etc/amnesia/version").exists()
        || Path::new("/live/persistence").exists()
}

/// Checks whether WebKitGTK compositing mode should be disabled for GPU/VM stability
/// (NVIDIA drivers, Qubes OS Xen display, Tails live GPU, or missing DRM render nodes).
#[must_use]
pub fn should_disable_webkit_compositing() -> bool {
    if detect_nvidia() || detect_qubes() {
        return true;
    }
    let has_dri_render = fs::read_dir("/dev/dri")
        .map(|rd| {
            rd.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("renderD"))
            })
        })
        .unwrap_or(false);
    !has_dri_render
}

/// Probes common local Tor SOCKS5 ports (`127.0.0.1:9050` / `9150`) with a 50ms timeout.
#[must_use]
pub fn detect_local_tor_socks5() -> Option<String> {
    for port in [9050u16, 9150u16] {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return Some(format!("socks5h://127.0.0.1:{port}"));
        }
    }
    None
}

/// Chooses the ordered list of official `yt-dlp` release asset names compatible with `(os, arch, libc, is_nixos)`.
#[must_use]
pub fn preferred_ytdlp_asset_names_for_os(
    os: &str,
    arch: &str,
    libc: &str,
    is_nixos: bool,
) -> Vec<&'static str> {
    if os == "windows" {
        return match arch {
            "x86" => vec!["yt-dlp_x86.exe", "yt-dlp.exe"],
            _ => vec!["yt-dlp.exe"],
        };
    }
    if libc == "musl" || is_nixos {
        return vec!["yt-dlp"];
    }
    match arch {
        "x86_64" => vec!["yt-dlp_linux", "yt-dlp"],
        "aarch64" => vec!["yt-dlp_linux_aarch64", "yt-dlp"],
        "arm" | "armv7l" => vec!["yt-dlp_linux_armv7l", "yt-dlp"],
        _ => vec!["yt-dlp"],
    }
}

/// Chooses the ordered list of official Linux `yt-dlp` release asset names compatible with `(arch, libc, is_nixos)`.
#[must_use]
pub fn preferred_ytdlp_asset_names(arch: &str, libc: &str, is_nixos: bool) -> Vec<&'static str> {
    preferred_ytdlp_asset_names_for_os("linux", arch, libc, is_nixos)
}

/// Inspects the running host environment (Linux or Windows) and returns a complete compatibility summary.
#[must_use]
pub fn detect_platform_info(paths: &AppPaths, settings: &Settings) -> LinuxPlatformInfo {
    #[cfg(target_os = "windows")]
    {
        let arch = std::env::consts::ARCH.to_string();
        let ytdlp_candidates = preferred_ytdlp_asset_names_for_os("windows", &arch, "msvc", false);
        let yt_dlp_asset = ytdlp_candidates
            .first()
            .copied()
            .unwrap_or("yt-dlp.exe")
            .to_string();
        let resolved_yt = paths.effective_yt_dlp(settings);
        let yt_dlp_path = if resolved_yt.exists() {
            Some(resolved_yt.to_string_lossy().into_owned())
        } else {
            None
        };
        let ffmpeg_path = paths
            .effective_ffmpeg(settings)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());

        LinuxPlatformInfo {
            distro_id: "windows".to_string(),
            distro_name: "Windows".to_string(),
            distro_like: vec!["win32".to_string()],
            arch,
            libc: if cfg!(target_env = "gnu") {
                "mingw".to_string()
            } else {
                "msvc".to_string()
            },
            init_system: "windows-services".to_string(),
            display_backend: "WebView2".to_string(),
            sandbox: "native".to_string(),
            is_immutable: false,
            has_nvidia: which::which("nvidia-smi").is_ok(),
            is_qubes: false,
            is_tails: false,
            recommended_tor_proxy: detect_local_tor_socks5(),
            yt_dlp_path,
            yt_dlp_asset,
            ffmpeg_path,
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let os_raw = fs::read_to_string("/etc/os-release")
            .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
            .unwrap_or_default();
        let (distro_id, distro_name, distro_like) = parse_os_release(&os_raw);
        let arch = std::env::consts::ARCH.to_string();
        let libc = detect_libc().to_string();
        let is_nixos = Path::new("/etc/NIXOS").exists();
        let (sandbox, is_immutable) = detect_sandbox_runtime(&distro_id);
        let is_tails = detect_tails(&distro_id);
        let is_qubes = detect_qubes();
        let recommended_tor_proxy = if is_tails || distro_id == "parrot" || is_qubes {
            detect_local_tor_socks5()
        } else {
            None
        };

        let ytdlp_candidates = preferred_ytdlp_asset_names(&arch, &libc, is_nixos);
        let yt_dlp_asset = ytdlp_candidates
            .first()
            .copied()
            .unwrap_or("yt-dlp")
            .to_string();

        let resolved_yt = paths.effective_yt_dlp(settings);
        let yt_dlp_path = if resolved_yt.exists() {
            Some(resolved_yt.to_string_lossy().into_owned())
        } else {
            None
        };
        let ffmpeg_path = paths
            .effective_ffmpeg(settings)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());

        LinuxPlatformInfo {
            distro_id,
            distro_name,
            distro_like,
            arch,
            libc,
            init_system: detect_init_system().to_string(),
            display_backend: detect_display_backend().to_string(),
            sandbox: sandbox.to_string(),
            is_immutable,
            has_nvidia: detect_nvidia(),
            is_qubes,
            is_tails,
            recommended_tor_proxy,
            yt_dlp_path,
            yt_dlp_asset,
            ffmpeg_path,
        }
    }
}
