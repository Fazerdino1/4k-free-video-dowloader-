//! XDG directory layout and TOML configuration persistence.
//!
//! Configuration lives in `~/.config/vidgrab/config.toml` and application data
//! (database, yt-dlp binary, browser profile, logs, archives) lives in
//! `~/.local/share/vidgrab/`.

use crate::error::{CoreError, CoreResult};
use std::fs;
use std::path::{Path, PathBuf};
use vidgrab_shared::Settings;

/// Resolves all XDG paths used by the application.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub browser_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub db_path: PathBuf,
    pub cookies_file: PathBuf,
    pub demucs_venv_dir: PathBuf,
}

impl AppPaths {
    /// Resolves XDG paths in memory without touching the filesystem (safe in read-only probes).
    #[must_use]
    pub fn resolve_without_creating() -> Self {
        let base_config =
            dirs::config_dir().unwrap_or_else(|| std::env::temp_dir().join("vidgrab-config"));
        let base_data =
            dirs::data_local_dir().unwrap_or_else(|| std::env::temp_dir().join("vidgrab-data"));
        let config_dir = base_config.join("vidgrab");
        let data_dir = base_data.join("vidgrab");
        Self {
            config_file: config_dir.join("config.toml"),
            config_dir,
            db_path: data_dir.join("vidgrab.sqlite3"),
            cookies_file: data_dir.join("webview_cookies.txt"),
            bin_dir: data_dir.join("bin"),
            logs_dir: data_dir.join("logs"),
            browser_dir: data_dir.join("browser"),
            archive_dir: data_dir.join("archives"),
            demucs_venv_dir: data_dir.join("demucs-venv"),
            data_dir,
        }
    }

    /// Initializes XDG paths for `vidgrab`, falling back to `$TMPDIR` on read-only containers.
    pub fn initialize() -> CoreResult<Self> {
        let base_config =
            dirs::config_dir().unwrap_or_else(|| std::env::temp_dir().join("vidgrab-config"));
        let base_data =
            dirs::data_local_dir().unwrap_or_else(|| std::env::temp_dir().join("vidgrab-data"));

        match Self::from_roots(&base_config.join("vidgrab"), &base_data.join("vidgrab")) {
            Ok(paths) => Ok(paths),
            Err(err) => {
                let fallback_root = std::env::temp_dir().join("vidgrab-runtime");
                if let Ok(paths) =
                    Self::from_roots(&fallback_root.join("config"), &fallback_root.join("data"))
                {
                    eprintln!(
                        "Attention : dossier XDG non inscriptible ({err}), repli sur {}",
                        fallback_root.display()
                    );
                    Ok(paths)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Initializes paths under explicit root directories (useful for isolated unit tests).
    pub fn from_roots(config_dir: &Path, data_dir: &Path) -> CoreResult<Self> {
        let bin_dir = data_dir.join("bin");
        let logs_dir = data_dir.join("logs");
        let browser_dir = data_dir.join("browser");
        let archive_dir = data_dir.join("archives");
        let demucs_venv_dir = data_dir.join("demucs-venv");

        for dir in [
            config_dir,
            data_dir,
            &bin_dir,
            &logs_dir,
            &browser_dir,
            &archive_dir,
        ] {
            fs::create_dir_all(dir)?;
        }

        Ok(Self {
            config_file: config_dir.join("config.toml"),
            config_dir: config_dir.to_path_buf(),
            db_path: data_dir.join("vidgrab.sqlite3"),
            cookies_file: data_dir.join("webview_cookies.txt"),
            data_dir: data_dir.to_path_buf(),
            bin_dir,
            logs_dir,
            browser_dir,
            archive_dir,
            demucs_venv_dir,
        })
    }

    /// Returns the path to the managed `yt-dlp` binary inside the local `bin` directory.
    pub fn managed_yt_dlp_path(&self) -> PathBuf {
        if cfg!(target_os = "windows") {
            self.bin_dir.join("yt-dlp.exe")
        } else {
            self.bin_dir.join("yt-dlp")
        }
    }

    /// Resolves the effective `yt-dlp` binary path based on user settings, portable sidecar, sandbox, and libc compatibility.
    pub fn effective_yt_dlp(&self, settings: &Settings) -> PathBuf {
        if let Some(custom) = &settings.custom_yt_dlp_path {
            if !custom.as_os_str().is_empty() {
                return custom.clone();
            }
        }
        if settings.use_system_yt_dlp {
            if let Ok(sys) = which::which("yt-dlp") {
                return sys;
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let sidecar = exe_dir.join(if cfg!(target_os = "windows") {
                    "yt-dlp.exe"
                } else {
                    "yt-dlp"
                });
                if sidecar.is_file() {
                    return sidecar;
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            for env_bin in [
                "/app/bin/yt-dlp".to_string(),
                std::env::var("SNAP")
                    .map(|s| format!("{s}/bin/yt-dlp"))
                    .unwrap_or_default(),
                std::env::var("SNAP")
                    .map(|s| format!("{s}/usr/bin/yt-dlp"))
                    .unwrap_or_default(),
                std::env::var("APPDIR")
                    .map(|s| format!("{s}/usr/bin/yt-dlp"))
                    .unwrap_or_default(),
            ] {
                if !env_bin.is_empty() {
                    let candidate = PathBuf::from(&env_bin);
                    if candidate.exists() {
                        return candidate;
                    }
                }
            }
            let musl_or_nixos =
                crate::platform::detect_libc() == "musl" || Path::new("/etc/NIXOS").exists();
            if musl_or_nixos {
                if let Ok(sys) = which::which("yt-dlp") {
                    return sys;
                }
            }
        }
        let managed = self.managed_yt_dlp_path();
        if managed.exists() {
            return managed;
        }
        which::which("yt-dlp").unwrap_or(managed)
    }

    /// Resolves the effective `ffmpeg` binary path across native, Windows portable/WinGet/Scoop/Choco,
    /// Flatpak, Snap, AppImage, NixOS, GNU Guix, and RPMFusion (`ffmpeg-free`) installations.
    pub fn effective_ffmpeg(&self, settings: &Settings) -> CoreResult<PathBuf> {
        if let Some(custom) = &settings.custom_ffmpeg_path {
            if custom.exists() {
                return Ok(custom.clone());
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let sidecar = exe_dir.join(if cfg!(target_os = "windows") {
                    "ffmpeg.exe"
                } else {
                    "ffmpeg"
                });
                if sidecar.is_file() {
                    return Ok(sidecar);
                }
            }
        }
        let managed_ffmpeg = self.bin_dir.join(if cfg!(target_os = "windows") {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        if managed_ffmpeg.is_file() {
            return Ok(managed_ffmpeg);
        }
        if let Ok(found) = which::which("ffmpeg") {
            return Ok(found);
        }
        if let Ok(found_free) = which::which("ffmpeg-free") {
            return Ok(found_free);
        }
        let mut candidates = Vec::new();
        #[cfg(target_os = "windows")]
        {
            if let Some(local_app) = dirs::data_local_dir() {
                candidates.push(local_app.join("Microsoft/WinGet/Links/ffmpeg.exe"));
            }
            if let Some(home) = dirs::home_dir() {
                candidates.push(home.join("scoop/shims/ffmpeg.exe"));
            }
            candidates.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin\ffmpeg.exe"));
            candidates.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
            candidates.push(PathBuf::from(r"C:\Program Files\ffmpeg\bin\ffmpeg.exe"));
        }
        #[cfg(not(target_os = "windows"))]
        {
            candidates.extend([
                PathBuf::from("/app/bin/ffmpeg"),
                PathBuf::from("/run/current-system/sw/bin/ffmpeg"),
                PathBuf::from("/usr/local/bin/ffmpeg"),
                PathBuf::from("/usr/bin/ffmpeg"),
                PathBuf::from("/usr/bin/ffmpeg-free"),
            ]);
            if let Some(snap) = std::env::var_os("SNAP") {
                let p = PathBuf::from(snap);
                candidates.push(p.join("usr/bin/ffmpeg"));
                candidates.push(p.join("bin/ffmpeg"));
            }
            if let Some(appdir) = std::env::var_os("APPDIR") {
                candidates.push(PathBuf::from(appdir).join("usr/bin/ffmpeg"));
            }
            if let Some(home) = dirs::home_dir() {
                candidates.push(home.join(".nix-profile/bin/ffmpeg"));
                candidates.push(home.join(".guix-profile/bin/ffmpeg"));
                candidates.push(home.join(".local/bin/ffmpeg"));
            }
        }
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .ok_or(CoreError::FfmpegMissing)
    }
}

/// Loads `Settings` from disk, or writes and returns defaults tailored to the detected Linux OS.
pub fn load_or_create_settings(paths: &AppPaths) -> CoreResult<Settings> {
    if paths.config_file.exists() {
        let raw = fs::read_to_string(&paths.config_file)?;
        let settings: Settings = toml::from_str(&raw)?;
        Ok(settings)
    } else {
        let mut settings = Settings::default();
        let fallback_home = dirs::home_dir().unwrap_or_else(|| paths.data_dir.clone());
        let video_base = dirs::video_dir().unwrap_or_else(|| fallback_home.join("Videos"));
        let audio_base = dirs::audio_dir().unwrap_or_else(|| fallback_home.join("Music"));
        settings.video_output_dir = video_base.join("4kFreeDownloader");
        settings.smart_mode.output_dir = settings.video_output_dir.clone();
        settings.audio_output_dir = audio_base.join("4kFreeDownloader");

        if crate::platform::detect_tails("") {
            if let Some(tor_proxy) = crate::platform::detect_local_tor_socks5() {
                settings.proxy.url = tor_proxy;
                settings.proxy.enabled = true;
            }
        }
        save_settings(paths, &settings)?;
        Ok(settings)
    }
}

/// Persists `Settings` to `~/.config/vidgrab/config.toml`.
pub fn save_settings(paths: &AppPaths, settings: &Settings) -> CoreResult<()> {
    if let Some(parent) = paths.config_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = toml::to_string_pretty(settings)?;
    fs::write(&paths.config_file, serialized)?;
    Ok(())
}
