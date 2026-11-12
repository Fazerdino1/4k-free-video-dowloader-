//! Subprocess execution of `yt-dlp` via `tokio::process`, real-time stdout progress
//! streaming, SIGTERM pause via `nix`, and partial file cleanup on cancellation.

pub mod args;
pub mod parser;

use crate::config::AppPaths;
use crate::error::{CoreError, CoreResult};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use vidgrab_shared::{DownloadProgress, DownloadRequest, DownloadStatus, Settings, VideoInfo};

const ANALYZE_CACHE_TTL: Duration = Duration::from_secs(600);
const ANALYZE_CACHE_MAX_ENTRIES: usize = 64;
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(45);

type AnalyzeCacheKey = (String, bool, Option<String>, Option<String>);
static ANALYZE_CACHE: OnceLock<Mutex<HashMap<AnalyzeCacheKey, (Instant, VideoInfo)>>> =
    OnceLock::new();

fn analyze_cache() -> &'static Mutex<HashMap<AnalyzeCacheKey, (Instant, VideoInfo)>> {
    ANALYZE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Analyzes a single video or playlist URL using `yt-dlp --dump-single-json`
/// with a bounded 10-minute in-memory cache and 45-second subprocess timeout.
pub async fn analyze_url(
    paths: &AppPaths,
    settings: &Settings,
    url: &str,
    flat_playlist: bool,
) -> CoreResult<VideoInfo> {
    if !url.starts_with("ytsearch") {
        CoreError::check_drm_url(url)?;
    }

    let cache_key: AnalyzeCacheKey = (
        url.trim().to_string(),
        flat_playlist,
        Some(format!("{:?}", settings.cookie_browser)),
        if settings.proxy.url.is_empty() {
            None
        } else {
            Some(settings.proxy.url.clone())
        },
    );
    if let Ok(guard) = analyze_cache().lock() {
        if let Some((cached_at, info)) = guard.get(&cache_key) {
            if cached_at.elapsed() < ANALYZE_CACHE_TTL {
                debug!("Serving analyze_url cache hit for {}", cache_key.0);
                return Ok(info.clone());
            }
        }
    }

    let yt_dlp = paths.effective_yt_dlp(settings);
    if !yt_dlp.exists() {
        return Err(CoreError::YtDlpMissing(
            yt_dlp.to_string_lossy().into_owned(),
        ));
    }

    let cmd_args = args::build_analyze_args(url, flat_playlist, settings);
    debug!("Running yt-dlp analyze: {:?} {:?}", yt_dlp, cmd_args);

    let fut = Command::new(&yt_dlp)
        .args(&cmd_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = tokio::time::timeout(ANALYZE_TIMEOUT, fut)
        .await
        .map_err(|_| {
            CoreError::YtDlpFailed(
                "Délai dépassé (45 s) lors de l'analyse du lien par yt-dlp".to_string(),
            )
        })??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::from_yt_dlp_stderr(&stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let info = parser::parse_video_info(&stdout)?;

    if let Ok(mut guard) = analyze_cache().lock() {
        guard.retain(|_, (ts, _)| ts.elapsed() < ANALYZE_CACHE_TTL);
        if guard.len() >= ANALYZE_CACHE_MAX_ENTRIES {
            guard.clear();
        }
        guard.insert(cache_key, (Instant::now(), info.clone()));
    }

    Ok(info)
}

/// Lists all extractors supported by the installed `yt-dlp` binary (`yt-dlp --list-extractors`).
pub async fn list_extractors(paths: &AppPaths, settings: &Settings) -> CoreResult<Vec<String>> {
    let yt_dlp = paths.effective_yt_dlp(settings);
    if !yt_dlp.exists() {
        return Err(CoreError::YtDlpMissing(
            yt_dlp.to_string_lossy().into_owned(),
        ));
    }

    let output = Command::new(&yt_dlp)
        .arg("--list-extractors")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::YtDlpFailed(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let list = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    Ok(list)
}

/// Sends `SIGTERM` to the `yt-dlp` process group (including any child `ffmpeg`)
/// to cleanly pause it while preserving `.part` files.
pub fn terminate_process(pid: u32) -> CoreResult<()> {
    let pgid = Pid::from_raw(-(pid as i32));
    let direct = Pid::from_raw(pid as i32);
    kill(pgid, Signal::SIGTERM)
        .or_else(|_| kill(direct, Signal::SIGTERM))
        .map_err(|e| CoreError::YtDlpFailed(format!("Failed to send SIGTERM to PID {pid}: {e}")))
}

/// Removes `.part` and `.ytdl` temporary files in `output_dir` matching the download title or ID.
pub fn cleanup_partial_files(output_dir: &Path, title_or_id: &str) -> CoreResult<usize> {
    if !output_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    let entries = std::fs::read_dir(output_dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let is_partial =
                name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag");
            if is_partial
                && (title_or_id.is_empty() || name.contains(title_or_id))
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Spawns a `yt-dlp` download subprocess, streaming progress updates over `progress_tx`.
/// Also registers the child PID via `pid_callback` so the queue manager can pause/cancel it.
pub async fn run_download<F>(
    paths: &AppPaths,
    req: &DownloadRequest,
    settings: &Settings,
    extra_args: &[String],
    progress_tx: mpsc::UnboundedSender<DownloadProgress>,
    pid_callback: F,
) -> CoreResult<DownloadProgress>
where
    F: FnOnce(u32),
{
    if !req.url.starts_with("ytsearch") {
        CoreError::check_drm_url(&req.url)?;
    }

    // Ensure ffmpeg is available before starting video/audio merges
    let _ = paths.effective_ffmpeg(settings)?;

    let yt_dlp = paths.effective_yt_dlp(settings);
    if !yt_dlp.exists() {
        return Err(CoreError::YtDlpMissing(
            yt_dlp.to_string_lossy().into_owned(),
        ));
    }

    std::fs::create_dir_all(&req.output_dir)?;

    let mut cmd_args = args::build_args(req, settings);
    if !extra_args.is_empty() {
        // Insert extra args before the `-- <url>` tail
        let insert_pos = cmd_args.len().saturating_sub(2);
        for (i, arg) in extra_args.iter().enumerate() {
            cmd_args.insert(insert_pos + i, arg.clone());
        }
    }

    info!("Starting yt-dlp download {}: {:?}", req.id, cmd_args);

    let mut cmd = Command::new(&yt_dlp);
    cmd.args(&cmd_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;

    if let Some(pid) = child.id() {
        pid_callback(pid);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CoreError::YtDlpFailed("Failed to capture yt-dlp stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| CoreError::YtDlpFailed("Failed to capture yt-dlp stderr".to_string()))?;

    let req_id = req.id;
    let tx_clone = progress_tx.clone();

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut last_progress = DownloadProgress {
            id: req_id,
            status: DownloadStatus::Downloading,
            ..Default::default()
        };
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(path) = line.trim().strip_prefix(args::FINAL_PATH_PREFIX) {
                last_progress.output_path = Some(std::path::PathBuf::from(path.trim()));
                continue;
            }
            if let Some(mut prog) = parser::parse_progress_line(req_id, &line) {
                if prog.output_path.is_none() {
                    prog.output_path = last_progress.output_path.clone();
                }
                last_progress = prog.clone();
                let _ = tx_clone.send(prog);
            }
        }
        last_progress
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut err_buf = Vec::new();
        while let Ok(Some(line)) = reader.next_line().await {
            warn!("yt-dlp stderr: {}", line);
            err_buf.push(line);
        }
        err_buf.join("\n")
    });

    let status = child.wait().await?;
    let mut final_progress = stdout_task.await.unwrap_or_else(|_| DownloadProgress {
        id: req_id,
        ..Default::default()
    });
    let stderr_text = stderr_task.await.unwrap_or_default();

    if status.success() {
        final_progress.status = DownloadStatus::Completed;
        final_progress.percent = 100.0;
        let _ = progress_tx.send(final_progress.clone());
        Ok(final_progress)
    } else {
        let err = CoreError::from_yt_dlp_stderr(&stderr_text);
        final_progress.status = DownloadStatus::Failed;
        final_progress.error_message = Some(err.to_string());
        let _ = progress_tx.send(final_progress);
        Err(err)
    }
}
