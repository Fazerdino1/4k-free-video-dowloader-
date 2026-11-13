//! System tray icon, native notifications, and OS autostart integration.

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::AppState;
use std::sync::RwLock;
use vidgrab_shared::Settings;

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::path::PathBuf;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::menu::{Menu, MenuItem};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::tray::TrayIconBuilder;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{Emitter, Manager};

/// Configures OS login autostart (Windows Registry Run key or Linux XDG `.desktop`).
pub fn configure_autostart_desktop_entry(enabled: bool) -> std::io::Result<()> {
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
    #[cfg(not(any(target_os = "windows", target_os = "android", target_os = "ios")))]
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
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = enabled;
        Ok(())
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn embedded_app_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("./icons/128x128.png")
}

/// Sends a native notification (Android NotificationManager, Linux `notify-send`, or Windows balloon).
pub fn send_desktop_notification_if_enabled(
    settings: &RwLock<Settings>,
    summary: &str,
    body: &str,
) {
    let enabled = settings
        .read()
        .map(|s| s.notifications_enabled)
        .unwrap_or(true);
    if !enabled {
        return;
    }
    if let Some(bridge) = vidgrab_core::mobile_bridge::get() {
        let t = summary.to_string();
        let b = body.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = bridge.send_notification(t, b).await;
        });
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

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn setup_tray_icon(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
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
