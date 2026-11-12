//! Runtime bridge allowing mobile hosts (Android via Tauri 2 Kotlin plugin)
//! to execute `youtubedl-android`, `ffmpeg-android`, clipboard, notifications,
//! and MediaScanner integrations natively without spawning desktop subprocesses.

use crate::error::CoreResult;
use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Asynchronous engine backend implemented by mobile host targets (Android).
pub trait MobileEngineBridge: Send + Sync {
    /// Executes `yt-dlp` with arguments (typically `--dump-single-json` or `--list-extractors`)
    /// and returns trimmed stdout.
    fn run_yt_dlp_json(&self, args: Vec<String>) -> BoxFuture<'static, CoreResult<String>>;

    /// Runs a streaming `yt-dlp` download task, forwarding stdout lines to `line_tx`.
    fn run_yt_dlp_download(
        &self,
        task_id: String,
        args: Vec<String>,
        line_tx: mpsc::UnboundedSender<String>,
    ) -> BoxFuture<'static, CoreResult<()>>;

    /// Terminates an in-flight download task by `task_id` while preserving `.part` files.
    fn cancel_yt_dlp_task(&self, task_id: String) -> BoxFuture<'static, CoreResult<()>>;

    /// Runs `ffmpeg` with the supplied CLI argument list on the local device.
    fn run_ffmpeg(&self, args: Vec<String>) -> BoxFuture<'static, CoreResult<()>>;

    /// Updates the embedded `yt-dlp` engine via the mobile updater channel and returns the version/status.
    fn update_yt_dlp(&self) -> BoxFuture<'static, CoreResult<String>>;

    /// Returns the embedded `yt-dlp` version string.
    fn get_yt_dlp_version(&self) -> BoxFuture<'static, CoreResult<String>>;

    /// Triggers OS media indexing (`MediaScannerConnection`) so newly saved videos/audio appear in Gallery/Music.
    fn scan_media_file(&self, path: String) -> BoxFuture<'static, CoreResult<()>>;

    /// Reads plain text from the system clipboard.
    fn read_clipboard(&self) -> BoxFuture<'static, CoreResult<String>>;

    /// Writes plain text into the system clipboard.
    fn write_clipboard(&self, text: String) -> BoxFuture<'static, CoreResult<()>>;

    /// Opens a file or folder via native Android `Intent` / `FileProvider`.
    fn open_path(&self, path: String, reveal_parent: bool) -> BoxFuture<'static, CoreResult<()>>;

    /// Shows a native Android notification when downloads finish or fail.
    fn send_notification(&self, title: String, body: String) -> BoxFuture<'static, CoreResult<()>>;
}

static BRIDGE: OnceLock<Arc<dyn MobileEngineBridge>> = OnceLock::new();

/// Registers the process-wide mobile engine implementation during mobile startup.
pub fn register(bridge: Arc<dyn MobileEngineBridge>) {
    let _ = BRIDGE.set(bridge);
}

/// Returns the active mobile engine implementation if registered (Android).
#[must_use]
pub fn get() -> Option<Arc<dyn MobileEngineBridge>> {
    BRIDGE.get().cloned()
}

/// Returns `true` when running on a mobile host with an active [`MobileEngineBridge`].
#[must_use]
pub fn is_registered() -> bool {
    BRIDGE.get().is_some()
}

/// Fires an asynchronous cancellation of `id` on the mobile engine when registered.
pub fn cancel_task_if_registered(id: Uuid) {
    if let Some(bridge) = get() {
        let task_id = id.to_string();
        tokio::spawn(async move {
            let _ = bridge.cancel_yt_dlp_task(task_id).await;
        });
    }
}
