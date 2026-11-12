//! Frontend UI crate (`vidgrab-ui`) for **4k free video dowloader**.
//!
//! Written 100% in Rust. Can be compiled to `wasm32-unknown-unknown` with Trunk
//! or rendered directly into the Tauri 2 WebView.

pub mod components;
pub mod i18n;
pub mod pages;

#[cfg(test)]
mod tests {
    use super::components::{format_bytes, render_download_card};
    use super::i18n::I18nCatalog;
    use super::pages::render_app_shell;
    use chrono::Utc;
    use vidgrab_shared::{DownloadItem, DownloadProgress, DownloadRequest, Settings};

    #[test]
    fn test_ui_rendering_and_i18n() {
        let fr = I18nCatalog::new("fr");
        let en = I18nCatalog::new("en");
        assert_eq!(fr.t("toolbar-paste"), "Coller le lien");
        assert_eq!(en.t("toolbar-paste"), "Paste Link");

        let req = DownloadRequest::default();
        let item = DownloadItem {
            progress: DownloadProgress {
                id: req.id,
                percent: 75.0,
                ..Default::default()
            },
            request: req,
            created_at: Utc::now(),
            completed_at: None,
        };

        let card = render_download_card(&item, &fr);
        assert!(card.contains("aria-valuenow=\"75\""));
        assert_eq!(format_bytes(104_857_600), "100.0 MiB");

        let shell = render_app_shell(&[item], &Settings::default());
        assert!(shell.contains("4k free video dowloader"));
        assert!(shell.contains("btn-toggle-notifications"));
        assert!(shell.contains("Coller le lien"));

        let empty_shell = render_app_shell(&[], &Settings::default());
        let dist_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist");
        let _ = std::fs::create_dir_all(&dist_dir);
        let _ = std::fs::write(dist_dir.join("index.html"), empty_shell);
    }
}
