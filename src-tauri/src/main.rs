//! Tauri 2 desktop binary for **4k free video dowloader**.
//!
//! Exposes strongly-typed async Tauri commands for URL analysis, download queue
//! management, Smart Mode, subscriptions, built-in WebKitGTK browser with Netscape
//! cookie export (`0600`), AI audio processing (Demucs + `ffmpeg`), proxy testing,
//! system tray icon, autostart `.desktop` management, and rotating log files.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::{Timelike, Utc};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;
use vidgrab_core::ai_audio::{self, is_demucs_installed};
use vidgrab_core::config::{load_or_create_settings, save_settings, AppPaths};
use vidgrab_core::cookies::write_netscape_cookies_file;
use vidgrab_core::db::Database;
use vidgrab_core::engine;
use vidgrab_core::media_tools;
use vidgrab_core::platform;
use vidgrab_core::queue::DownloadQueue;
use vidgrab_core::smart_mode::build_smart_download_request;
use vidgrab_core::subscriptions::SubscriptionManager;
use vidgrab_core::updater::ensure_or_update_yt_dlp;
#[cfg(target_os = "linux")]
use vidgrab_shared::CookieBrowser;
use vidgrab_shared::{
    AiAudioOperation, CookieEntry, DownloadItem, DownloadProgress, DownloadRequest,
    LinuxPlatformInfo, MediaToolAction, PostDownloadAction, Settings, Subscription, VideoInfo,
};

/// Shared backend state managed by Tauri.
pub struct AppState {
    pub paths: AppPaths,
    pub db: Arc<Database>,
    pub settings: Arc<RwLock<Settings>>,
    pub queue: DownloadQueue,
    pub subscriptions: SubscriptionManager,
    pub logs: Arc<Mutex<VecDeque<String>>>,
    pub tray_available: Arc<AtomicBool>,
}

fn push_app_log(logs: &Mutex<VecDeque<String>>, app: Option<&AppHandle>, msg: impl AsRef<str>) {
    let ts = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{ts}] {}", msg.as_ref());
    if let Ok(mut guard) = logs.lock() {
        if guard.len() >= 400 {
            guard.pop_front();
        }
        guard.push_back(line.clone());
    }
    if let Some(handle) = app {
        let _ = handle.emit("app-log", &line);
    }
}

type CmdResult<T> = Result<T, String>;

#[tauri::command]
async fn cmd_get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())
}

#[tauri::command]
async fn cmd_save_settings(state: State<'_, AppState>, new_settings: Settings) -> CmdResult<()> {
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

#[tauri::command]
async fn cmd_run_media_tool(
    app: AppHandle,
    state: State<'_, AppState>,
    input_path: PathBuf,
    action: MediaToolAction,
) -> CmdResult<PathBuf> {
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Outil ffmpeg ({action:?}) : {}", input_path.display()),
    );
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let out_path = media_tools::run_ffmpeg_tool(&state.paths, &settings, &input_path, &action)
        .await
        .map_err(|e| {
            push_app_log(&state.logs, Some(&app), format!("Erreur ffmpeg : {e}"));
            e.to_string()
        })?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Outil ffmpeg terminé : {}", out_path.display()),
    );
    Ok(out_path)
}

#[tauri::command]
async fn cmd_analyze_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    flat_playlist: bool,
) -> CmdResult<VideoInfo> {
    push_app_log(&state.logs, Some(&app), format!("Analyse : {url}"));
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let mut info = match engine::analyze_url(&state.paths, &settings, &url, flat_playlist).await {
        Ok(i) => i,
        Err(e) => {
            push_app_log(&state.logs, Some(&app), format!("Erreur analyse : {e}"));
            return Err(e.to_string());
        }
    };
    const MAX_PREFETCH_ENTRY_THUMBS: usize = 30;
    let mut entry_tasks = Vec::new();
    for (idx, entry) in info
        .entries
        .iter()
        .take(MAX_PREFETCH_ENTRY_THUMBS)
        .enumerate()
    {
        let thumb_url = entry.thumbnail.clone();
        let eid = entry.id.clone();
        let eurl = entry.effective_url();
        entry_tasks.push((
            idx,
            tokio::spawn(async move {
                vidgrab_core::fetch_thumbnail_with_fallback(thumb_url.as_deref(), &eid, &eurl).await
            }),
        ));
    }

    let top_thumb_url = info.thumbnail.clone();
    let top_id = info.id.clone();
    let top_url = info.webpage_url.clone().unwrap_or_else(|| url.clone());
    let top_data =
        vidgrab_core::fetch_thumbnail_with_fallback(top_thumb_url.as_deref(), &top_id, &top_url)
            .await;

    for (idx, handle) in entry_tasks {
        if let Ok(Some(data_uri)) = handle.await {
            if let Some(entry) = info.entries.get_mut(idx) {
                entry.thumbnail = Some(data_uri);
            }
        }
    }

    if let Some(data_uri) = top_data {
        info.thumbnail = Some(data_uri);
    } else if info.thumbnail.is_none() {
        info.thumbnail = info.entries.first().and_then(|e| e.thumbnail.clone());
    }
    push_app_log(
        &state.logs,
        Some(&app),
        format!(
            "Analysé : {} ({} format(s), {} entrée(s))",
            info.title,
            info.formats.len(),
            info.entries.len()
        ),
    );
    Ok(info)
}

#[tauri::command]
async fn cmd_paste_or_smart_download(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<DownloadItem>> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;

    if settings.smart_mode.enabled {
        let info = engine::analyze_url(&state.paths, &settings, &url, false)
            .await
            .map_err(|e| e.to_string())?;
        let req = build_smart_download_request(&info, &settings);
        let item = state.queue.enqueue(req).map_err(|e| e.to_string())?;
        Ok(Some(item))
    } else {
        Ok(None)
    }
}

#[tauri::command]
async fn cmd_enqueue_download(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DownloadRequest,
) -> CmdResult<DownloadItem> {
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Ajout file : {} [{}]", request.title, request.url),
    );
    state.queue.enqueue(request).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_import_bulk_file(
    state: State<'_, AppState>,
    file_path: PathBuf,
) -> CmdResult<Vec<String>> {
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;
    let urls = vidgrab_core::parse_bulk_urls(&content);
    let _ = &state.paths;
    Ok(urls)
}

#[tauri::command]
async fn cmd_list_downloads(state: State<'_, AppState>) -> CmdResult<Vec<DownloadItem>> {
    Ok(state.queue.list_items())
}

#[tauri::command]
async fn cmd_pause_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.pause(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_resume_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.resume(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_cancel_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.cancel(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_remove_download(
    state: State<'_, AppState>,
    id: Uuid,
    delete_file: bool,
) -> CmdResult<()> {
    state
        .queue
        .remove(id, delete_file)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_open_path(path: PathBuf) -> CmdResult<()> {
    // Fall back to the closest existing parent folder if the file was moved/renamed
    let mut target = path.clone();
    while !target.exists() {
        match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => target = p.to_path_buf(),
            _ => break,
        }
    }
    open::that(&target).map_err(|e| format!("Impossible d'ouvrir {} : {e}", target.display()))
}

/// Reads the system clipboard (PowerShell on Windows, `wl-paste`/`xclip`/`xsel` on Linux).
#[tauri::command]
async fn cmd_read_clipboard() -> CmdResult<String> {
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
    #[cfg(not(target_os = "windows"))]
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
}

/// Writes text to the system clipboard (PowerShell on Windows, `wl-copy`/`xclip` on Linux).
#[tauri::command]
async fn cmd_write_clipboard(text: String) -> CmdResult<()> {
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
        Err("Impossible de copier dans le presse-papiers Windows".to_string())
    }
    #[cfg(not(target_os = "windows"))]
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
}

#[tauri::command]
async fn cmd_export_downloads_json(
    state: State<'_, AppState>,
    dest_path: PathBuf,
) -> CmdResult<()> {
    let json = state.db.export_json().map_err(|e| e.to_string())?;
    fs::write(&dest_path, json).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_import_downloads_json(
    state: State<'_, AppState>,
    src_path: PathBuf,
) -> CmdResult<(usize, usize)> {
    let json = fs::read_to_string(&src_path).map_err(|e| e.to_string())?;
    let counts = state.db.import_json(&json).map_err(|e| e.to_string())?;
    state.queue.reload_from_db().map_err(|e| e.to_string())?;
    Ok(counts)
}

#[tauri::command]
async fn cmd_list_subscriptions(state: State<'_, AppState>) -> CmdResult<Vec<Subscription>> {
    state
        .subscriptions
        .list_subscriptions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_save_subscription(
    state: State<'_, AppState>,
    subscription: Subscription,
) -> CmdResult<()> {
    state
        .subscriptions
        .save_subscription(&subscription)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_remove_subscription(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state
        .subscriptions
        .remove_subscription(id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_check_subscription_now(
    state: State<'_, AppState>,
    id: Uuid,
) -> CmdResult<Vec<DownloadItem>> {
    state
        .subscriptions
        .check_subscription_now(id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_update_yt_dlp(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
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
async fn cmd_list_extractors(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
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
async fn cmd_test_proxy(proxy_url: String) -> CmdResult<u128> {
    vidgrab_core::test_proxy_connection(&proxy_url, "https://www.youtube.com")
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_check_demucs(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(is_demucs_installed(&state.paths))
}

#[tauri::command]
async fn cmd_install_demucs(state: State<'_, AppState>) -> CmdResult<String> {
    ai_audio::install_demucs_venv(&state.paths)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_run_ai_audio(
    state: State<'_, AppState>,
    input_path: PathBuf,
    operation: AiAudioOperation,
) -> CmdResult<PathBuf> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    ai_audio::process_audio_ai(&state.paths, &settings, &input_path, operation)
        .await
        .map_err(|e| e.to_string())
}

const BROWSER_HUD_SCRIPT: &str = r#"(function() {
  if (window.top !== window.self) return;
  function mount() {
    if (document.getElementById('vg-browser-hud')) return;
    const host = document.createElement('div');
    host.id = 'vg-browser-hud';
    host.style.cssText = 'position:fixed;top:10px;left:50%;transform:translateX(-50%);z-index:2147483647;display:flex;align-items:center;gap:6px;padding:6px 10px;background:rgba(18,19,22,0.94);border:1px solid #10b981;border-radius:10px;box-shadow:0 10px 30px rgba(0,0,0,0.65);font-family:Inter,system-ui,sans-serif;font-size:12px;color:#f3f4f6;backdrop-filter:blur(8px);max-width:94vw;';
    const btnCss = 'background:#23252a;color:#f3f4f6;border:1px solid #2e3138;border-radius:6px;padding:5px 9px;cursor:pointer;font-weight:600;white-space:nowrap;';
    const urlInp = document.createElement('input');
    urlInp.type = 'text';
    urlInp.value = location.href;
    urlInp.style.cssText = 'width:320px;max-width:40vw;background:#141518;color:#f3f4f6;border:1px solid #2e3138;border-radius:6px;padding:5px 8px;font-size:12px;outline:none;';
    urlInp.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        let v = urlInp.value.trim();
        if (!v) return;
        if (!/^https?:\/\//i.test(v)) v = 'https://' + v;
        location.href = v;
      }
    });
    const mkBtn = (label, title, bg, cb) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      b.title = title;
      b.style.cssText = btnCss + (bg ? ('background:' + bg + ';border-color:' + bg + ';color:#fff;') : '');
      b.addEventListener('click', cb);
      return b;
    };
    host.appendChild(mkBtn('◀', 'Précédent', '', () => history.back()));
    host.appendChild(mkBtn('▶', 'Suivant', '', () => history.forward()));
    host.appendChild(mkBtn('↻', 'Actualiser', '', () => location.reload()));
    host.appendChild(urlInp);
    host.appendChild(mkBtn('⬇ Télécharger cette page', 'Envoyer cette page vers 4k free video dowloader', '#10b981', () => {
      location.assign('https://vidgrab-action.local/download?url=' + encodeURIComponent(location.href));
    }));
    host.appendChild(mkBtn('🍪 Cookies', 'Synchroniser les cookies de session pour les vidéos privées', '', () => {
      location.assign('https://vidgrab-action.local/sync-cookies?url=' + encodeURIComponent(location.href));
    }));
    host.appendChild(mkBtn('✕', 'Masquer la barre', '', () => { host.style.display = 'none'; }));
    (document.body || document.documentElement).appendChild(host);
    let lastHref = location.href;
    setInterval(() => {
      if (location.href !== lastHref && document.activeElement !== urlInp) {
        lastHref = location.href;
        urlInp.value = lastHref;
      }
    }, 800);
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', mount);
  else mount();
})();"#;

#[cfg(target_os = "linux")]
fn export_webkit_cookies_for_url(
    browser_win: &tauri::WebviewWindow,
    target_url: String,
    cookies_file: PathBuf,
    logs: Arc<Mutex<VecDeque<String>>>,
    app_handle: AppHandle,
    enable_internal_cookies_if_none: bool,
) {
    let _ = browser_win.with_webview(move |pw| {
        use webkit2gtk::{CookieManagerExt, WebViewExt, WebsiteDataManagerExt};
        let wv = pw.inner();
        let Some(dm) = wv.website_data_manager() else {
            return;
        };
        let Some(cm) = dm.cookie_manager() else {
            return;
        };
        let url_clone = target_url.clone();
        cm.cookies(
            &target_url,
            None::<&webkit2gtk::gio::Cancellable>,
            move |res| match res {
                Ok(cookies) => {
                    let mut entries = Vec::with_capacity(cookies.len());
                    for mut c in cookies {
                        let domain = c.domain().map(|s| s.to_string()).unwrap_or_default();
                        let name = c.name().map(|s| s.to_string()).unwrap_or_default();
                        let value = c.value().map(|s| s.to_string()).unwrap_or_default();
                        let path = c
                            .path()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "/".to_string());
                        let secure = c.is_secure();
                        let expires = c.expires().map(|dt| dt.to_unix()).unwrap_or(0);
                        if !domain.is_empty() && !name.is_empty() {
                            entries.push(CookieEntry {
                                include_subdomains: domain.starts_with('.'),
                                domain,
                                path,
                                secure,
                                expires,
                                name,
                                value,
                            });
                        }
                    }
                    let count = entries.len();
                    if let Err(e) = write_netscape_cookies_file(&cookies_file, &entries) {
                        push_app_log(
                            &logs,
                            Some(&app_handle),
                            format!("Erreur export cookies : {e}"),
                        );
                        return;
                    }
                    if enable_internal_cookies_if_none && count > 0 {
                        let state = app_handle.state::<AppState>();
                        if let Ok(mut guard) = state.settings.write() {
                            if guard.cookie_browser == CookieBrowser::None {
                                guard.cookie_browser = CookieBrowser::InternalWebview;
                                let _ = save_settings(&state.paths, &guard);
                            }
                        };
                    }
                    push_app_log(
                        &logs,
                        Some(&app_handle),
                        format!("Cookies navigateur exportés : {count} cookie(s) pour {url_clone}"),
                    );
                    let _ = app_handle.emit("browser-cookies-synced", count);
                }
                Err(e) => {
                    push_app_log(
                        &logs,
                        Some(&app_handle),
                        format!("Lecture cookies WebKitGTK : {e}"),
                    );
                }
            },
        );
    });
}

/// Opens the built-in WebKitGTK browser window with persistent profile in
/// `~/.local/share/vidgrab/browser/` and a floating control HUD bar.
#[tauri::command]
async fn cmd_open_builtin_browser(
    app: AppHandle,
    state: State<'_, AppState>,
    initial_url: Option<String>,
) -> CmdResult<()> {
    let target = initial_url.unwrap_or_else(|| "https://www.youtube.com".to_string());
    let parsed: url::Url = target.parse().map_err(|e| format!("Invalid URL: {e}"))?;

    if let Some(existing) = app.get_webview_window("browser") {
        let _ = existing.navigate(parsed);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let browser_data_dir = state.paths.browser_dir.clone();
    let app_for_nav = app.clone();
    let mut win_builder = WebviewWindowBuilder::new(&app, "browser", WebviewUrl::External(parsed))
        .title("4k free video dowloader — Navigateur intégré")
        .inner_size(1140.0, 760.0)
        .data_directory(browser_data_dir);
    win_builder = win_builder
        .icon(embedded_app_icon())
        .map_err(|e| e.to_string())?;
    let _win = win_builder
        .initialization_script(BROWSER_HUD_SCRIPT)
        .on_navigation(move |nav_url| {
            if nav_url.host_str() != Some("vidgrab-action.local") {
                return true;
            }
            let target_url = nav_url
                .query_pairs()
                .find(|(k, _)| k == "url")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let path = nav_url.path().to_string();
            let handle = app_for_nav.clone();
            let main_handle = handle.clone();
            let _ = main_handle.run_on_main_thread(move || {
                #[cfg(target_os = "linux")]
                if let Some(bw) = handle.get_webview_window("browser") {
                    if !target_url.is_empty() {
                        let st = handle.state::<AppState>();
                        export_webkit_cookies_for_url(
                            &bw,
                            target_url.clone(),
                            st.paths.cookies_file.clone(),
                            st.logs.clone(),
                            handle.clone(),
                            path == "/sync-cookies",
                        );
                    }
                }
                if path == "/download" && !target_url.is_empty() {
                    if let Some(main_w) = handle.get_webview_window("main") {
                        let _ = main_w.show();
                        let _ = main_w.unminimize();
                        let _ = main_w.set_focus();
                    }
                    let _ = handle.emit("browser-download-url", &target_url);
                }
            });
            false
        })
        .build()
        .map_err(|e| format!("Failed to open browser window: {e}"))?;

    Ok(())
}

/// Exports session cookies from the built-in browser to `~/.local/share/vidgrab/webview_cookies.txt`
/// in Netscape format with `0600` permissions.
#[tauri::command]
async fn cmd_export_browser_cookies(
    state: State<'_, AppState>,
    cookies: Vec<CookieEntry>,
) -> CmdResult<PathBuf> {
    write_netscape_cookies_file(&state.paths.cookies_file, &cookies).map_err(|e| e.to_string())?;
    Ok(state.paths.cookies_file.clone())
}

#[tauri::command]
async fn cmd_get_logs(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    Ok(state
        .logs
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default())
}

#[tauri::command]
async fn cmd_clear_logs(state: State<'_, AppState>) -> CmdResult<()> {
    if let Ok(mut g) = state.logs.lock() {
        g.clear();
    }
    Ok(())
}

#[tauri::command]
async fn cmd_get_disk_space(path: PathBuf) -> CmdResult<Option<(u64, u64)>> {
    Ok(vidgrab_core::get_disk_space_bytes(&path))
}

#[tauri::command]
async fn cmd_pause_all(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.pause_all().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Pause globale ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
async fn cmd_resume_all(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.resume_all().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Reprise globale ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
async fn cmd_retry_failed(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.retry_failed().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Relance des échecs ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
async fn cmd_clear_finished(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.clear_finished().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Nettoyage de la liste ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
async fn cmd_prioritize_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.prioritize(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_schedule_override_now(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.queue.set_schedule_bypass(true);
    push_app_log(
        &state.logs,
        Some(&app),
        "Planificateur horaire contourné manuellement",
    );
    Ok(())
}

#[tauri::command]
async fn cmd_get_schedule_status(state: State<'_, AppState>) -> CmdResult<bool> {
    let s = state
        .settings
        .read()
        .map(|g| g.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let hour = chrono::Local::now().hour() as u8;
    Ok(s.schedule_enabled && !vidgrab_core::is_within_schedule(&s, hour))
}

#[tauri::command]
async fn cmd_check_all_subscriptions(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<usize> {
    let subs = state
        .subscriptions
        .list_subscriptions()
        .map_err(|e| e.to_string())?;
    let mut total_added = 0usize;
    for sub in subs {
        if sub.paused {
            continue;
        }
        if let Ok(items) = state.subscriptions.check_subscription_now(sub.id).await {
            total_added += items.len();
        }
    }
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Vérification abonnements terminée : {total_added} nouvelle(s) vidéo(s)"),
    );
    Ok(total_added)
}

#[tauri::command]
async fn cmd_clean_subtitles_to_txt(file_path: PathBuf) -> CmdResult<PathBuf> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let sub_source = if matches!(ext.as_str(), "srt" | "vtt" | "ass") && file_path.exists() {
        file_path
    } else {
        // Locate a matching sidecar subtitle file (`<stem>.<lang>.vtt|srt|ass`) in the same folder
        let stem = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut found = None;
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let e2 = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();
                if !stem.is_empty()
                    && name.starts_with(&stem)
                    && matches!(e2.as_str(), "srt" | "vtt" | "ass")
                {
                    found = Some(p);
                    break;
                }
            }
        }
        found.ok_or_else(|| {
            "Aucun fichier sous-titre (.srt / .vtt / .ass) trouvé pour cet élément.".to_string()
        })?
    };

    let raw = fs::read_to_string(&sub_source)
        .map_err(|e| format!("Lecture sous-titres impossible : {e}"))?;
    let cleaned = vidgrab_core::engine::parser::clean_subtitles_to_txt(&raw);
    if cleaned.is_empty() {
        return Err("Le fichier sous-titres ne contenait aucun texte exploitable.".to_string());
    }
    let out_txt = sub_source.with_extension("txt");
    fs::write(&out_txt, cleaned).map_err(|e| format!("Écriture .txt impossible : {e}"))?;
    Ok(out_txt)
}

/// Returns runtime Linux distribution, architecture, libc, init, sandbox, display, and Tor details.
#[tauri::command]
async fn cmd_get_platform_info(state: State<'_, AppState>) -> CmdResult<LinuxPlatformInfo> {
    let settings = state
        .settings
        .read()
        .map_err(|_| "Settings lock poisoned".to_string())?
        .clone();
    Ok(platform::detect_platform_info(&state.paths, &settings))
}

/// Triggers system shutdown across Windows (`shutdown /s /t 5`) and Linux (`systemd`, `elogind`, `OpenRC`, `runit`, `sysvinit`).
#[tauri::command]
async fn cmd_shutdown_system() -> CmdResult<()> {
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
    #[cfg(not(target_os = "windows"))]
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
}

/// Configures OS login autostart (Windows Registry Run key or Linux XDG `.desktop`).
fn configure_autostart_desktop_entry(enabled: bool) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let value_name = "4k free video dowloader";
        if enabled {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("vidgrab.exe"));
            let quoted = format!("\"{}\"", exe.display());
            let _ = std::process::Command::new("reg.exe")
                .args([
                    "add", run_key, "/v", value_name, "/t", "REG_SZ", "/d", &quoted, "/f",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .status()?;
        } else {
            let _ = std::process::Command::new("reg.exe")
                .args(["delete", run_key, "/v", value_name, "/f"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Some(cfg_dir) = dirs::config_dir() else {
            return Ok(());
        };
        let autostart_dir = cfg_dir.join("autostart");
        fs::create_dir_all(&autostart_dir)?;
        let desktop_file = autostart_dir.join("vidgrab.desktop");
        if enabled {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/bin/vidgrab"));
            let content = format!(
                "[Desktop Entry]\nType=Application\nName=4k free video dowloader\nExec={}\nIcon=4k-free-video-dowloader\nStartupWMClass=vidgrab\nTerminal=false\nCategories=AudioVideo;Video;\n",
                exe.display()
            );
            fs::write(desktop_file, content)?;
        } else if desktop_file.exists() {
            let _ = fs::remove_file(desktop_file);
        }
        Ok(())
    }
}

/// Returns the embedded high-resolution green cloud application icon (`128x128` PNG).
fn embedded_app_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("./icons/128x128.png")
}

/// Sends a native desktop notification (via `notify-send` on Linux or PowerShell balloon on Windows) when enabled in settings.
fn send_desktop_notification_if_enabled(settings: &RwLock<Settings>, summary: &str, body: &str) {
    let enabled = settings
        .read()
        .map(|s| s.notifications_enabled)
        .unwrap_or(true);
    if !enabled {
        return;
    }
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("notify-send")
        .args([
            "--app-name=4k free video dowloader",
            "--icon=4k-free-video-dowloader",
            summary,
            body,
        ])
        .spawn();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let title_esc = summary.replace('\'', "''");
        let body_esc = body.replace('\'', "''");
        let ps_script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $n=New-Object System.Windows.Forms.NotifyIcon; $n.Icon=[System.Drawing.SystemIcons]::Information; $n.Visible=$true; $n.ShowBalloonTip(5000,'{title_esc}','{body_esc}',[System.Windows.Forms.ToolTipIcon]::Info); Start-Sleep -Seconds 6; $n.Dispose()"
        );
        let _ = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &ps_script,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = (summary, body);
}

/// Rotates and initializes log file in `~/.local/share/vidgrab/logs/vidgrab.log`.
fn init_rotating_log(logs_dir: &Path) {
    let _ = fs::create_dir_all(logs_dir);
    let log_file = logs_dir.join("vidgrab.log");
    if let Ok(meta) = fs::metadata(&log_file) {
        if meta.len() > 5 * 1024 * 1024 {
            let rotated = logs_dir.join("vidgrab.log.1");
            let _ = fs::rename(&log_file, rotated);
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_file) {
        let _ = writeln!(
            f,
            "[{}] 4k free video dowloader started",
            Utc::now().to_rfc3339()
        );
    }
}

fn setup_tray_icon(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(
        app,
        "show",
        "Afficher 4k free video dowloader",
        true,
        None::<&str>,
    )?;
    let paste_item = MenuItem::with_id(app, "paste", "Coller un lien", true, None::<&str>)?;
    let pause_all_item =
        MenuItem::with_id(app, "pause_all", "Tout mettre en pause", true, None::<&str>)?;
    let resume_all_item =
        MenuItem::with_id(app, "resume_all", "Tout reprendre", true, None::<&str>)?;
    let check_subs = MenuItem::with_id(
        app,
        "check_subs",
        "Vérifier les abonnements maintenant",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &paste_item,
            &pause_all_item,
            &resume_all_item,
            &check_subs,
            &quit_item,
        ],
    )?;

    let mut tray_builder = TrayIconBuilder::new()
        .menu(&menu)
        .on_menu_event(|app_handle, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
            "paste" => {
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
                let _ = app_handle.emit("trigger-paste", ());
            }
            "pause_all" => {
                let st = app_handle.state::<AppState>();
                let _ = st.queue.pause_all();
            }
            "resume_all" => {
                let st = app_handle.state::<AppState>();
                let _ = st.queue.resume_all();
            }
            "check_subs" => {
                let h = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let st = h.state::<AppState>();
                    if let Ok(subs) = st.subscriptions.list_subscriptions() {
                        let mut added = 0usize;
                        for sub in subs.into_iter().filter(|s| !s.paused) {
                            if let Ok(items) = st.subscriptions.check_subscription_now(sub.id).await
                            {
                                added += items.len();
                            }
                        }
                        send_desktop_notification_if_enabled(
                            &st.settings,
                            "Abonnements vérifiés",
                            &format!("{added} nouvelle(s) vidéo(s) ajoutée(s)."),
                        );
                    }
                });
            }
            "quit" => {
                app_handle.exit(0);
            }
            _ => {}
        });
    tray_builder = tray_builder.icon(embedded_app_icon());
    let _tray = tray_builder.build(app)?;
    Ok(())
}

fn main() {
    let cli_args: Vec<String> = std::env::args().collect();
    if cli_args.len() > 1 {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            if let Some(exit_code) =
                rt.block_on(vidgrab_core::cli::try_run_cli_from_args(&cli_args))
            {
                std::process::exit(exit_code);
            }
        }
    }

    #[cfg(target_os = "linux")]
    if platform::detect_display_backend() == "headless" {
        eprintln!(
            "Aucun serveur d'affichage graphique détecté (WAYLAND_DISPLAY / DISPLAY absents)."
        );
        eprintln!("Utilisez le mode ligne de commande sans interface graphique :");
        vidgrab_core::cli::print_usage(
            cli_args.first().map(String::as_str).unwrap_or("vidgrab"),
            true,
        );
        std::process::exit(1);
    }

    // Stabilize WebKitGTK 4.1 across NVIDIA proprietary drivers, Wayland/XWayland, Qubes/Tails, and VMs
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none()
            && platform::should_disable_webkit_compositing()
        {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
        if std::env::var_os("GDK_BACKEND").is_none() {
            let has_wayland = std::env::var_os("WAYLAND_DISPLAY")
                .filter(|v| !v.is_empty())
                .is_some();
            let has_x11 = std::env::var_os("DISPLAY")
                .filter(|v| !v.is_empty())
                .is_some();
            let backend = if has_wayland && !has_x11 {
                "wayland,x11"
            } else {
                "x11,wayland"
            };
            std::env::set_var("GDK_BACKEND", backend);
        }
        if std::env::var_os("GTK_THEME").is_none() {
            std::env::set_var("GTK_THEME", "Adwaita:dark");
        }
    }

    let paths = match AppPaths::initialize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Fatal: failed to initialize XDG directories: {e}");
            std::process::exit(1);
        }
    };

    init_rotating_log(&paths.logs_dir);

    let settings = match load_or_create_settings(&paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Warning: could not read settings ({e}), using defaults");
            Settings::default()
        }
    };

    let db = match Database::open(&paths.db_path) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("Fatal: failed to open SQLite database: {e}");
            std::process::exit(1);
        }
    };

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let shared_settings = Arc::new(RwLock::new(settings.clone()));

    let queue = match DownloadQueue::new(
        paths.clone(),
        db.clone(),
        shared_settings.clone(),
        progress_tx,
    ) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Fatal: failed to initialize DownloadQueue: {e}");
            std::process::exit(1);
        }
    };

    let subscriptions = SubscriptionManager::new(
        paths.clone(),
        db.clone(),
        queue.clone(),
        shared_settings.clone(),
    );

    // Render initial HTML shell into dist/index.html so the WebView always displays the latest state
    let initial_html = vidgrab_ui::pages::render_app_shell(&queue.list_items(), &settings);
    let dist_index = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("crates/vidgrab-ui/dist/index.html"));
    if let Some(idx_path) = dist_index {
        if idx_path.parent().map(|d| d.exists()).unwrap_or(false) {
            let _ = fs::write(idx_path, initial_html);
        }
    }

    let subs_for_setup = subscriptions.clone();
    let app_logs = Arc::new(Mutex::new(VecDeque::with_capacity(128)));
    push_app_log(
        &app_logs,
        None,
        "4k free video dowloader démarré (yt-dlp + ffmpeg)",
    );

    let tray_available = Arc::new(AtomicBool::new(false));
    let app_state = AppState {
        paths: paths.clone(),
        db,
        settings: shared_settings,
        queue,
        subscriptions,
        logs: app_logs,
        tray_available: tray_available.clone(),
    };

    let builder_result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    let st = window.state::<AppState>();
                    let tray_ok = st.tray_available.load(Ordering::Relaxed);
                    let minimize = st
                        .settings
                        .read()
                        .map(|s| s.minimize_to_tray)
                        .unwrap_or(false);
                    if tray_ok && minimize {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .setup(move |app| {
            if let Some(main_win) = app.get_webview_window("main") {
                let _ = main_win.set_icon(embedded_app_icon());
            }
            let tray_ok = setup_tray_icon(app).is_ok();
            app.state::<AppState>()
                .tray_available
                .store(tray_ok, Ordering::Relaxed);

            // Start the periodic subscription scheduler inside Tauri's active Tokio runtime
            let subs_runner = subs_for_setup.clone();
            let subs_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                subs_runner.spawn_scheduler(move |sub, new_items| {
                    let st = subs_handle.state::<AppState>();
                    send_desktop_notification_if_enabled(
                        &st.settings,
                        &format!("Nouvelle(s) vidéo(s) — {}", sub.title),
                        &format!(
                            "{} vidéo(s) ajoutée(s) à la file d'attente.",
                            new_items.len()
                        ),
                    );
                });
            });

            // Forward real-time download progress to the frontend WebView via Tauri events
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut notified = std::collections::HashSet::new();
                while let Some(prog) = progress_rx.recv().await {
                    let st = handle.state::<AppState>();
                    if prog.status == vidgrab_shared::DownloadStatus::Completed
                        && notified.insert(prog.id)
                    {
                        let name = prog
                            .output_path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Fichier".to_string());
                        push_app_log(&st.logs, Some(&handle), format!("Terminé : {name}"));
                        let (notif_ok, action) = st
                            .settings
                            .read()
                            .map(|s| (s.notifications_enabled, s.post_download_action))
                            .unwrap_or((true, PostDownloadAction::Notify));
                        if notif_ok
                            && matches!(
                                action,
                                PostDownloadAction::Notify | PostDownloadAction::ShutdownSystem
                            )
                        {
                            send_desktop_notification_if_enabled(
                                &st.settings,
                                "Téléchargement terminé ✓",
                                &name,
                            );
                        }
                        if action == PostDownloadAction::ShutdownSystem
                            && !st.queue.has_active_or_queued()
                        {
                            let _ = handle.emit("shutdown-scheduled", ());
                        }
                    } else if prog.status == vidgrab_shared::DownloadStatus::Failed {
                        if let Some(err) = &prog.error_message {
                            push_app_log(
                                &st.logs,
                                Some(&handle),
                                format!("Échec {} : {err}", prog.id),
                            );
                        }
                    }
                    let _ = handle.emit("download-progress", &prog);
                }
            });

            // Auto-update or install yt-dlp on startup if enabled
            if settings.auto_update_yt_dlp {
                let paths_clone = paths.clone();
                let log_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let st = log_handle.state::<AppState>();
                    match ensure_or_update_yt_dlp(&paths_clone, false).await {
                        Ok(ver) => {
                            info!("yt-dlp ready: {ver}");
                            push_app_log(
                                &st.logs,
                                Some(&log_handle),
                                format!("yt-dlp prêt : {ver}"),
                            );
                        }
                        Err(err) => {
                            eprintln!("yt-dlp startup check notice: {err}");
                            push_app_log(
                                &st.logs,
                                Some(&log_handle),
                                format!("Info yt-dlp : {err}"),
                            );
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_get_settings,
            cmd_save_settings,
            cmd_analyze_url,
            cmd_paste_or_smart_download,
            cmd_enqueue_download,
            cmd_import_bulk_file,
            cmd_list_downloads,
            cmd_pause_download,
            cmd_resume_download,
            cmd_cancel_download,
            cmd_remove_download,
            cmd_open_path,
            cmd_read_clipboard,
            cmd_write_clipboard,
            cmd_export_downloads_json,
            cmd_import_downloads_json,
            cmd_list_subscriptions,
            cmd_save_subscription,
            cmd_remove_subscription,
            cmd_check_subscription_now,
            cmd_check_all_subscriptions,
            cmd_update_yt_dlp,
            cmd_list_extractors,
            cmd_test_proxy,
            cmd_check_demucs,
            cmd_install_demucs,
            cmd_run_ai_audio,
            cmd_open_builtin_browser,
            cmd_export_browser_cookies,
            cmd_shutdown_system,
            cmd_get_logs,
            cmd_clear_logs,
            cmd_get_disk_space,
            cmd_pause_all,
            cmd_resume_all,
            cmd_retry_failed,
            cmd_clear_finished,
            cmd_prioritize_download,
            cmd_schedule_override_now,
            cmd_get_schedule_status,
            cmd_clean_subtitles_to_txt,
            cmd_run_media_tool,
            cmd_get_platform_info,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = builder_result {
        eprintln!("Error running Tauri application: {e}");
        std::process::exit(1);
    }
}
