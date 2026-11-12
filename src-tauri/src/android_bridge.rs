//! Android native plugin bridge connecting `vidgrab-core` (`MobileEngineBridge`)
//! to `io.github.vidgrab.VidGrabAndroidPlugin` (`youtubedl-android` + `ffmpeg-android`).

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::plugin::{Builder as PluginBuilder, PluginHandle, TauriPlugin};
use tauri::Manager;
use tokio::sync::mpsc;
use vidgrab_core::error::CoreError;
use vidgrab_core::mobile_bridge::MobileEngineBridge;

/// Bridge backed by Tauri 2's registered Kotlin [`VidGrabAndroidPlugin`].
pub struct AndroidBridge {
    handle: PluginHandle<tauri::Wry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextResp {
    #[serde(default)]
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineEvent {
    #[serde(default)]
    line: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareResp {
    url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArgsPayload {
    args: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadPayload {
    task_id: String,
    args: Vec<String>,
    on_line: Channel<InvokeResponseBody>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskPayload {
    task_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PathPayload {
    path: String,
    reveal_parent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextPayload {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationPayload {
    title: String,
    body: String,
}

impl AndroidBridge {
    /// Creates a new Android engine bridge bound to `handle`.
    #[must_use]
    pub fn new(handle: PluginHandle<tauri::Wry>) -> Self {
        Self { handle }
    }

    /// Consumes a shared URL passed to the Android app via `Intent.ACTION_SEND`, if any.
    pub async fn take_pending_shared_url(&self) -> Result<Option<String>, String> {
        let resp: ShareResp = self
            .handle
            .run_mobile_plugin_async("takePendingSharedUrl", serde_json::json!({}))
            .await
            .map_err(|e| format!("Android share bridge error: {e}"))?;
        Ok(resp.url.filter(|u| !u.trim().is_empty()))
    }
}

/// Initializes the Tauri 2 mobile plugin that binds `io.github.vidgrab.VidGrabAndroidPlugin`
/// and registers it as the process-wide [`MobileEngineBridge`].
pub fn init_plugin() -> TauriPlugin<tauri::Wry> {
    PluginBuilder::new("vidgrab-android")
        .setup(|app, api| {
            let handle =
                api.register_android_plugin("io.github.vidgrab", "VidGrabAndroidPlugin")?;
            let bridge = Arc::new(AndroidBridge::new(handle));
            vidgrab_core::mobile_bridge::register(bridge.clone());
            app.manage(bridge);
            Ok(())
        })
        .build()
}

impl MobileEngineBridge for AndroidBridge {
    fn run_yt_dlp_json(&self, args: Vec<String>) -> BoxFuture<'static, Result<String, CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let resp: TextResp = handle
                .run_mobile_plugin_async("runYtDlpJson", ArgsPayload { args })
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android yt-dlp: {e}")))?;
            Ok(resp.value)
        })
    }

    fn run_yt_dlp_download(
        &self,
        task_id: String,
        args: Vec<String>,
        line_tx: mpsc::UnboundedSender<String>,
    ) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let tx_for_chan = line_tx.clone();
            let channel = Channel::new(move |body| {
                if let InvokeResponseBody::Json(json) = body {
                    if let Ok(evt) = serde_json::from_str::<LineEvent>(&json) {
                        let _ = tx_for_chan.send(evt.line);
                    }
                }
                Ok(())
            });
            let _: serde_json::Value = handle
                .run_mobile_plugin_async(
                    "runYtDlpDownload",
                    DownloadPayload {
                        task_id,
                        args,
                        on_line: channel,
                    },
                )
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android yt-dlp download: {e}")))?;
            Ok(())
        })
    }

    fn cancel_yt_dlp_task(&self, task_id: String) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async("cancelTask", TaskPayload { task_id })
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android cancel: {e}")))?;
            Ok(())
        })
    }

    fn run_ffmpeg(&self, args: Vec<String>) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async("runFfmpeg", ArgsPayload { args })
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android ffmpeg: {e}")))?;
            Ok(())
        })
    }

    fn update_yt_dlp(&self) -> BoxFuture<'static, Result<String, CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let resp: TextResp = handle
                .run_mobile_plugin_async("updateYtDlp", serde_json::json!({}))
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android update yt-dlp: {e}")))?;
            Ok(resp.value)
        })
    }

    fn get_yt_dlp_version(&self) -> BoxFuture<'static, Result<String, CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let resp: TextResp = handle
                .run_mobile_plugin_async("getYtDlpVersion", serde_json::json!({}))
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android version yt-dlp: {e}")))?;
            Ok(resp.value)
        })
    }

    fn scan_media_file(&self, path: String) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async(
                    "scanMediaFile",
                    PathPayload {
                        path,
                        reveal_parent: false,
                    },
                )
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android media scan: {e}")))?;
            Ok(())
        })
    }

    fn read_clipboard(&self) -> BoxFuture<'static, Result<String, CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let resp: TextResp = handle
                .run_mobile_plugin_async("readClipboard", serde_json::json!({}))
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android clipboard read: {e}")))?;
            Ok(resp.value)
        })
    }

    fn write_clipboard(&self, text: String) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async("writeClipboard", TextPayload { text })
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android clipboard write: {e}")))?;
            Ok(())
        })
    }

    fn open_path(
        &self,
        path: String,
        reveal_parent: bool,
    ) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async(
                    "openPath",
                    PathPayload {
                        path,
                        reveal_parent,
                    },
                )
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android open path: {e}")))?;
            Ok(())
        })
    }

    fn send_notification(
        &self,
        title: String,
        body: String,
    ) -> BoxFuture<'static, Result<(), CoreError>> {
        let handle = self.handle.clone();
        Box::pin(async move {
            let _: serde_json::Value = handle
                .run_mobile_plugin_async("sendNotification", NotificationPayload { title, body })
                .await
                .map_err(|e| CoreError::YtDlpFailed(format!("Android notification: {e}")))?;
            Ok(())
        })
    }
}
