//! Built-in desktop browser and Netscape cookie synchronization commands.

use crate::commands::CmdResult;
use crate::AppState;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use vidgrab_core::cookies::write_netscape_cookies_file;
use vidgrab_shared::CookieEntry;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::tray::embedded_app_icon;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(target_os = "linux")]
use crate::logging::push_app_log;
#[cfg(target_os = "linux")]
use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use vidgrab_core::config::save_settings;
#[cfg(target_os = "linux")]
use vidgrab_shared::CookieBrowser;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
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

/// Opens the built-in browser window on desktop, or prompts/opens Share intent hint on Android.
#[tauri::command]
pub async fn cmd_open_builtin_browser(
    app: AppHandle,
    state: State<'_, AppState>,
    initial_url: Option<String>,
) -> CmdResult<()> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let _ = (app, state);
        if let (Some(bridge), Some(url)) = (vidgrab_core::mobile_bridge::get(), initial_url) {
            let _ = bridge.open_path(url, false).await;
        }
        return Err(
            "Sur Android, partagez directement une vidéo depuis YouTube / TikTok / Chrome via Partager → 4k free video dowloader."
                .to_string(),
        );
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
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
        let mut win_builder =
            WebviewWindowBuilder::new(&app, "browser", WebviewUrl::External(parsed))
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
}

/// Exports session cookies to `webview_cookies.txt` in Netscape format with `0600` permissions.
#[tauri::command]
pub async fn cmd_export_browser_cookies(
    state: State<'_, AppState>,
    cookies: Vec<CookieEntry>,
) -> CmdResult<PathBuf> {
    write_netscape_cookies_file(&state.paths.cookies_file, &cookies).map_err(|e| e.to_string())?;
    Ok(state.paths.cookies_file.clone())
}
