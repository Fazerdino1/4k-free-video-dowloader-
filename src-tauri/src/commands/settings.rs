//! Settings, clipboard, system info, yt-dlp updater, and OS integration commands.

use crate::commands::CmdResult;
use crate::logging::push_app_log;
use crate::tray::configure_autostart_desktop_entry;
use crate::AppState;
use std::path::PathBuf;
use tauri::{AppHandle, State};
#[cfg(target_os = "android")]
use tauri::Manager;
use vidgrab_core::config::save_settings;
use vidgrab_core::engine;
use vidgrab_core::platform;
use vidgrab_core::updater::ensure_or_update_yt_dlp;
use vidgrab_shared::{LinuxPlatformInfo, Settings};

#[tauri::command]
pub async fn cmd_get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())
}

#[tauri::command]
pub async fn cmd_save_settings(state: State<'_, AppState>, new_settings: Settings) -> CmdResult<()> {
    save_settings(&state.paths, &new_settings).map_err(|e| e.to_string())?;
    let _ = configure_autostart_desktop_entry(new_settings.autostart);
    state
        .queue
        .update_concurrency(new_settings.concurrent_downloads);
    if let Ok(mut guard) = state.settings.write() {
        *guard = new_settings;
    }
    Ok(())
}

/// Updates `speed_limit_kbps` at runtime from the toolbar without replacing other settings.
#[tauri::command]
pub async fn cmd_set_speed_limit(
    state: State<'_, AppState>,
    speed_limit_kbps: u64,
) -> CmdResult<()> {
    let updated = {
        let mut guard = state
            .settings
            .write()
            .map_err(|_| "Settings lock poisoned".to_string())?;
        guard.speed_limit_kbps = speed_limit_kbps;
        guard.clone()
    };
    save_settings(&state.paths, &updated).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_open_path(path: PathBuf) -> CmdResult<()> {
    if let Some(bridge) = vidgrab_core::mobile_bridge::get() {
        return bridge
            .open_path(path.to_string_lossy().into_owned(), path.is_dir())
            .await
            .map_err(|e| e.to_string());
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let mut target = path.clone();
        while !target.exists() {
            match target.parent() {
                Some(p) if !p.as_os_str().is_empty() => target = p.to_path_buf(),
                _ => break,
            }
        }
        open::that(&target).map_err(|e| format!("Impossible d'ouvrir {} : {e}", target.display()))
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = path;
        Err("Ouverture directe indisponible".to_string())
    }
}

/// Reads the system clipboard (Android ClipboardManager, Windows PowerShell, or Linux wl-paste/xclip).
#[tauri::command]
pub async fn cmd_read_clipboard() -> CmdResult<String> {
    if let Some(bridge) = vidgrab_core::mobile_bridge::get() {
        return bridge.read_clipboard().await.map_err(|e| e.to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; Get-Clipboard -Raw",
        ]);
        vidgrab_core::configure_background_command(&mut cmd);
        if let Ok(out) = cmd.output().await {
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
            }
        }
        Err("Presse-papiers Windows inaccessible".to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "android", target_os = "ios")))]
    {
        let candidates: [(&str, &[&str]); 3] = [
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
        ];
        for (bin, args) in candidates {
            if let Ok(out) = tokio::process::Command::new(bin).args(args).output().await {
                if out.status.success() {
                    return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
                }
            }
        }
        Err(
            "Presse-papiers inaccessible (installez wl-clipboard ou xclip via apt/dnf/pacman/zypper/apk)"
                .to_string(),
        )
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    Err("Presse-papiers mobile non initialisé".to_string())
}

/// Writes text to the system clipboard (Android ClipboardManager, Windows PowerShell, or Linux wl-copy/xclip).
#[tauri::command]
pub async fn cmd_write_clipboard(text: String) -> CmdResult<()> {
    if let Some(bridge) = vidgrab_core::mobile_bridge::get() {
        return bridge
            .write_clipboard(text)
            .await
            .map_err(|e| e.to_string());
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    use tokio::io::AsyncWriteExt;
    #[cfg(target_os = "windows")]
    {
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::InputEncoding=[System.Text.Encoding]::UTF8; $v=[Console]::In.ReadToEnd(); Set-Clipboard -Value $v",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        vidgrab_core::configure_background_command(&mut cmd);
        if let Ok(mut child) = cmd.spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes()).await;
            }
            if let Ok(st) = child.wait().await {
                if st.success() {
                    return Ok(());
                }
            }
        }
        return Err("Impossible de copier dans le presse-papiers Windows".to_string());
    }
    #[cfg(not(any(target_os = "windows", target_os = "android", target_os = "ios")))]
    {
        let candidates: [(&str, &[&str]); 2] = [
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard", "-i"]),
        ];
        for (bin, args) in candidates {
            if let Ok(mut child) = tokio::process::Command::new(bin)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes()).await;
                }
                if let Ok(st) = child.wait().await {
                    if st.success() {
                        return Ok(());
                    }
                }
            }
        }
        Err("Impossible de copier dans le presse-papiers".to_string())
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = text;
        Err("Presse-papiers mobile non initialisé".to_string())
    }
}

/// Consumes any URL passed to Android via the native OS Share Sheet (`ACTION_SEND`).
#[tauri::command]
pub async fn cmd_take_pending_share_url(app: AppHandle) -> CmdResult<Option<String>> {
    #[cfg(target_os = "android")]
    if let Some(bridge) = app.try_state::<std::sync::Arc<crate::android_bridge::AndroidBridge>>() {
        return bridge.take_pending_shared_url().await;
    }
    let _ = app;
    Ok(None)
}

#[tauri::command]
pub async fn cmd_update_yt_dlp(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
    push_app_log(
        &state.logs,
        Some(&app),
        "Vérification / mise à jour de yt-dlp...",
    );
    match ensure_or_update_yt_dlp(&state.paths, true).await {
        Ok(ver) => {
            push_app_log(&state.logs, Some(&app), format!("yt-dlp prêt : {ver}"));
            Ok(ver)
        }
        Err(e) => {
            push_app_log(&state.logs, Some(&app), format!("Erreur yt-dlp : {e}"));
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn cmd_list_extractors(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    engine::list_extractors(&state.paths, &settings)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_test_proxy(proxy_url: String) -> CmdResult<u128> {
    vidgrab_core::test_proxy_connection(&proxy_url, "https://www.youtube.com")
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_get_disk_space(path: PathBuf) -> CmdResult<Option<(u64, u64)>> {
    Ok(vidgrab_core::get_disk_space_bytes(&path))
}

/// Returns runtime OS, distribution, architecture, libc, init, sandbox, display, and Tor details.
#[tauri::command]
pub async fn cmd_get_platform_info(state: State<'_, AppState>) -> CmdResult<LinuxPlatformInfo> {
    let settings = state
        .settings
        .read()
        .map_err(|_| "Settings lock poisoned".to_string())?
        .clone();
    Ok(platform::detect_platform_info(&state.paths, &settings))
}

/// Triggers system shutdown across Windows (`shutdown /s /t 5`) and Linux (`systemd`, `elogind`, `OpenRC`, `runit`, `sysvinit`).
#[tauri::command]
pub async fn cmd_shutdown_system() -> CmdResult<()> {
    #[cfg(target_os = "android")]
    {
        return Err("L'extinction système du PC n'est pas applicable sur Android".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = tokio::process::Command::new("shutdown");
        cmd.args(["/s", "/t", "5"]);
        vidgrab_core::configure_background_command(&mut cmd);
        if let Ok(status) = cmd.status().await {
            if status.success() {
                return Ok(());
            }
        }
        Err("Extinction Windows impossible (shutdown /s)".to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "android", target_os = "ios")))]
    {
        let candidates: [(&str, &[&str]); 4] = [
            ("systemctl", &["poweroff"]),
            ("loginctl", &["poweroff"]),
            ("poweroff", &[]),
            ("shutdown", &["-h", "now"]),
        ];
        for (bin, args) in candidates {
            if let Ok(status) = tokio::process::Command::new(bin).args(args).status().await {
                if status.success() {
                    return Ok(());
                }
            }
        }
        Err(
            "Extinction système impossible (systemctl/loginctl/poweroff indisponibles ou droits insuffisants)"
                .to_string(),
        )
    }
    #[cfg(target_os = "ios")]
    Err("L'extinction système n'est pas applicable sur mobile".to_string())
}
