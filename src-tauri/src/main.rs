//! Tauri 2 desktop binary entrypoint for **4k free video dowloader**.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use vidgrab_core::platform;

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

    vidgrab_lib::run();
}
