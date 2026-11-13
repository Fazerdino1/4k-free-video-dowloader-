//! Shared Tauri 2 desktop & mobile runtime for **4k free video dowloader**.

#[cfg(target_os = "android")]
mod android_bridge;
mod commands;
mod logging;
mod tray;

use logging::{init_rotating_log, push_app_log};
use std::collections::VecDeque;
#[cfg(debug_assertions)]
use std::fs;
#[cfg(any(debug_assertions, target_os = "android"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tracing::info;
use tray::send_desktop_notification_if_enabled;
use vidgrab_core::config::{load_or_create_settings_with_dirs, AppPaths};
use vidgrab_core::db::Database;
use vidgrab_core::queue::DownloadQueue;
use vidgrab_core::subscriptions::SubscriptionManager;
use vidgrab_core::updater::ensure_or_update_yt_dlp;
use vidgrab_shared::{DownloadProgress, PostDownloadAction, Settings};

/// Shared backend state managed by Tauri across desktop and Android.
pub struct AppState {
    pub paths: AppPaths,
    pub db: Arc<Database>,
    pub settings: Arc<RwLock<Settings>>,
    pub queue: DownloadQueue,
    pub subscriptions: SubscriptionManager,
    pub logs: Arc<Mutex<VecDeque<String>>>,
    pub tray_available: Arc<AtomicBool>,
}

/// Initializes paths for desktop (XDG) or mobile (Tauri `App::path()` sandbox directories).
fn init_app_paths(app: &tauri::App) -> Result<AppPaths, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let cfg = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| PathBuf::from("/data/data/io.github.vidgrab.VidGrab/files/config"));
        let data = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("/data/data/io.github.vidgrab.VidGrab/files/data"));
        AppPaths::from_roots(&cfg, &data)
            .or_else(|_| {
                let fallback = PathBuf::from("/data/data/io.github.vidgrab.VidGrab/files");
                AppPaths::from_roots(&fallback.join("config"), &fallback.join("data"))
            })
            .map_err(|e| format!("Impossible d'initialiser les dossiers de l'application: {e}"))
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        AppPaths::initialize().map_err(|e| e.to_string())
    }
}

/// Entrypoint shared across Linux, Windows, macOS, and Android (`#[tauri::mobile_entry_point]`).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("Rust panic: {info}\n");
        eprintln!("{msg}");
        #[cfg(target_os = "android")]
        {
            use std::ffi::CString;
            if let Ok(c_tag) = CString::new("VidGrabCrash") {
                if let Ok(c_msg) = CString::new(msg.clone()) {
                    unsafe {
                        extern "C" {
                            fn __android_log_write(
                                prio: i32,
                                tag: *const std::os::raw::c_char,
                                text: *const std::os::raw::c_char,
                            ) -> i32;
                        }
                        __android_log_write(6, c_tag.as_ptr(), c_msg.as_ptr());
                    }
                }
            }
            for candidate in [
                "/data/user/0/io.github.vidgrab.VidGrab/files/vidgrab_crash.txt",
                "/data/data/io.github.vidgrab.VidGrab/files/vidgrab_crash.txt",
            ] {
                let _ = std::fs::write(candidate, &msg);
            }
        }
    }));

    #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "android")]
    {
        builder = builder.plugin(android_bridge::init_plugin());
    }

    let builder_result = builder
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
            let paths = init_app_paths(app)?;
            init_rotating_log(&paths.logs_dir);

            let settings = match load_or_create_settings_with_dirs(&paths, None, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: could not read settings ({e}), using defaults");
                    Settings::default()
                }
            };

            let db = Arc::new(
                Database::open(&paths.db_path)
                    .map_err(|e| format!("Fatal: failed to open SQLite database: {e}"))?,
            );

            let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DownloadProgress>();
            let shared_settings = Arc::new(RwLock::new(settings.clone()));

            let queue = DownloadQueue::new(
                paths.clone(),
                db.clone(),
                shared_settings.clone(),
                progress_tx,
            )
            .map_err(|e| format!("Fatal: failed to initialize DownloadQueue: {e}"))?;

            let subscriptions = SubscriptionManager::new(
                paths.clone(),
                db.clone(),
                queue.clone(),
                shared_settings.clone(),
            );

            #[cfg(all(debug_assertions, not(any(target_os = "android", target_os = "ios"))))]
            {
                let initial_html =
                    vidgrab_ui::pages::render_app_shell(&queue.list_items(), &settings);
                let dist_index = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .map(|p| p.join("crates/vidgrab-ui/dist/index.html"));
                if let Some(idx_path) = dist_index {
                    if idx_path.parent().map(|d| d.exists()).unwrap_or(false) {
                        let _ = fs::write(idx_path, initial_html);
                    }
                }
            }

            let app_logs = Arc::new(Mutex::new(VecDeque::with_capacity(128)));
            push_app_log(
                &app_logs,
                Some(app.handle()),
                "4k free video dowloader démarré (yt-dlp + ffmpeg)",
            );

            let tray_available = Arc::new(AtomicBool::new(false));
            let app_state = AppState {
                paths: paths.clone(),
                db,
                settings: shared_settings,
                queue,
                subscriptions: subscriptions.clone(),
                logs: app_logs,
                tray_available: tray_available.clone(),
            };
            app.manage(app_state);

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                if let Some(main_win) = app.get_webview_window("main") {
                    let _ = main_win.set_icon(tray::embedded_app_icon());
                }
                let tray_ok = tray::setup_tray_icon(app).is_ok();
                app.state::<AppState>()
                    .tray_available
                    .store(tray_ok, Ordering::Relaxed);
            }

            let subs_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                subscriptions.spawn_scheduler(move |sub, new_items| {
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
                            && !cfg!(target_os = "android")
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
            commands::settings::cmd_get_settings,
            commands::settings::cmd_save_settings,
            commands::settings::cmd_set_speed_limit,
            commands::downloads::cmd_analyze_url,
            commands::downloads::cmd_paste_or_smart_download,
            commands::downloads::cmd_enqueue_download,
            commands::downloads::cmd_import_bulk_file,
            commands::downloads::cmd_list_downloads,
            commands::downloads::cmd_search_downloads,
            commands::downloads::cmd_pause_download,
            commands::downloads::cmd_resume_download,
            commands::downloads::cmd_cancel_download,
            commands::downloads::cmd_remove_download,
            commands::settings::cmd_open_path,
            commands::settings::cmd_read_clipboard,
            commands::settings::cmd_write_clipboard,
            commands::settings::cmd_take_pending_share_url,
            commands::downloads::cmd_export_downloads_json,
            commands::downloads::cmd_import_downloads_json,
            commands::subscriptions::cmd_list_subscriptions,
            commands::subscriptions::cmd_save_subscription,
            commands::subscriptions::cmd_remove_subscription,
            commands::subscriptions::cmd_check_subscription_now,
            commands::subscriptions::cmd_check_all_subscriptions,
            commands::settings::cmd_update_yt_dlp,
            commands::settings::cmd_list_extractors,
            commands::settings::cmd_test_proxy,
            commands::media::cmd_check_demucs,
            commands::media::cmd_install_demucs,
            commands::media::cmd_run_ai_audio,
            commands::browser::cmd_open_builtin_browser,
            commands::browser::cmd_export_browser_cookies,
            commands::settings::cmd_shutdown_system,
            logging::cmd_get_logs,
            logging::cmd_clear_logs,
            commands::settings::cmd_get_disk_space,
            commands::downloads::cmd_pause_all,
            commands::downloads::cmd_resume_all,
            commands::downloads::cmd_retry_failed,
            commands::downloads::cmd_clear_finished,
            commands::downloads::cmd_prioritize_download,
            commands::downloads::cmd_schedule_override_now,
            commands::downloads::cmd_get_schedule_status,
            commands::media::cmd_clean_subtitles_to_txt,
            commands::media::cmd_run_media_tool,
            commands::media::cmd_run_whisper_transcribe,
            commands::settings::cmd_get_platform_info,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = builder_result {
        eprintln!("Error running Tauri application: {e}");
        std::process::exit(1);
    }
}
