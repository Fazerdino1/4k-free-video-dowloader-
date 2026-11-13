use crate::config::AppPaths;
use crate::error::{CoreError, CoreResult};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use vidgrab_shared::{MediaToolAction, Settings};

/// Validates and normalizes a user-supplied timestamp (`SS`, `MM:SS`, or `HH:MM:SS`, optional `.mmm`).
/// Returns `None` for empty strings or inputs containing invalid characters/segments.
#[must_use]
pub fn sanitize_timestamp(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (base, frac) = match trimmed.split_once('.') {
        Some((b, f)) => {
            if f.is_empty() || f.len() > 3 || !f.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            (b, Some(f))
        }
        None => (trimmed, None),
    };

    let parts: Vec<&str> = base.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }

    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let val: u64 = part.parse().ok()?;
        // Minutes and seconds must be < 60 when colon-separated.
        let is_sub_unit = parts.len() > 1 && idx > 0;
        if is_sub_unit && val >= 60 {
            return None;
        }
    }

    match frac {
        Some(f) => Some(format!("{base}.{f}")),
        None => Some(base.to_string()),
    }
}

/// Builds a `yt-dlp --download-sections` expression (`*start-end`, `*start-inf`, or `*0-end`).
#[must_use]
pub fn build_download_section_expr(
    clip_start: Option<&str>,
    clip_end: Option<&str>,
) -> Option<String> {
    let start = clip_start.and_then(sanitize_timestamp);
    let end = clip_end.and_then(sanitize_timestamp);

    match (start, end) {
        (Some(s), Some(e)) => Some(format!("*{s}-{e}")),
        (Some(s), None) => Some(format!("*{s}-inf")),
        (None, Some(e)) => Some(format!("*0-{e}")),
        (None, None) => None,
    }
}

/// Computes a non-colliding destination file path alongside `input_path`.
#[must_use]
pub fn build_output_path(input_path: &Path, action: &MediaToolAction) -> PathBuf {
    let parent = input_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("media");
    let orig_ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .unwrap_or("mp4");

    let (suffix, ext) = match action {
        MediaToolAction::RemuxMp4 => ("remux", "mp4"),
        MediaToolAction::ExtractAudioMp3 => ("audio", "mp3"),
        MediaToolAction::ExtractAudioFlac => ("audio", "flac"),
        MediaToolAction::CompressH264 => ("h264", "mp4"),
        MediaToolAction::TrimClip { .. } => ("clip", orig_ext),
    };

    let candidate = parent.join(format!("{stem}_{suffix}.{ext}"));
    if !candidate.exists() {
        return candidate;
    }

    for idx in 1u32..=999 {
        let numbered = parent.join(format!("{stem}_{suffix}_{idx}.{ext}"));
        if !numbered.exists() {
            return numbered;
        }
    }
    candidate
}

/// Constructs `ffmpeg` CLI arguments for a given [`MediaToolAction`].
pub fn build_ffmpeg_tool_args(
    input_path: &Path,
    output_path: &Path,
    action: &MediaToolAction,
) -> CoreResult<Vec<String>> {
    let src = input_path.to_string_lossy().into_owned();
    let dst = output_path.to_string_lossy().into_owned();

    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
    ];

    match action {
        MediaToolAction::RemuxMp4 => {
            args.extend([
                "-i".into(),
                src,
                "-c".into(),
                "copy".into(),
                "-movflags".into(),
                "+faststart".into(),
                dst,
            ]);
        }
        MediaToolAction::ExtractAudioMp3 => {
            args.extend([
                "-i".into(),
                src,
                "-vn".into(),
                "-c:a".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                "320k".into(),
                "-map_metadata".into(),
                "0".into(),
                dst,
            ]);
        }
        MediaToolAction::ExtractAudioFlac => {
            args.extend([
                "-i".into(),
                src,
                "-vn".into(),
                "-c:a".into(),
                "flac".into(),
                "-map_metadata".into(),
                "0".into(),
                dst,
            ]);
        }
        MediaToolAction::CompressH264 => {
            args.extend([
                "-i".into(),
                src,
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "medium".into(),
                "-crf".into(),
                "23".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
                "-movflags".into(),
                "+faststart".into(),
                dst,
            ]);
        }
        MediaToolAction::TrimClip { start, end } => {
            let clean_start = sanitize_timestamp(start);
            let clean_end = sanitize_timestamp(end);
            if clean_start.is_none() && clean_end.is_none() {
                return Err(CoreError::YtDlpFailed(
                    "Borne de découpe invalide (format attendu : MM:SS ou HH:MM:SS)".into(),
                ));
            }
            if let Some(s) = clean_start {
                args.extend(["-ss".into(), s]);
            }
            if let Some(e) = clean_end {
                args.extend(["-to".into(), e]);
            }
            args.extend([
                "-i".into(),
                src,
                "-c".into(),
                "copy".into(),
                "-avoid_negative_ts".into(),
                "make_zero".into(),
                dst,
            ]);
        }
    }

    Ok(args)
}

/// Runs an offline `ffmpeg` toolbox operation on a local media file and returns the output path.
pub async fn run_ffmpeg_tool(
    paths: &AppPaths,
    settings: &Settings,
    input_path: &Path,
    action: &MediaToolAction,
) -> CoreResult<PathBuf> {
    if !input_path.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::NotFound,
            format!("Fichier source introuvable : {}", input_path.display()),
        )
        .into());
    }

    let output_path = build_output_path(input_path, action);
    let args = build_ffmpeg_tool_args(input_path, &output_path, action)?;

    if let Some(bridge) = crate::mobile_bridge::get() {
        if let Err(err) = bridge.run_ffmpeg(args).await {
            let _ = tokio::fs::remove_file(&output_path).await;
            return Err(err);
        }
        let _ = bridge
            .scan_media_file(output_path.to_string_lossy().into_owned())
            .await;
        return Ok(output_path);
    }

    let ffmpeg_bin = paths.effective_ffmpeg(settings)?;

    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut cmd);
    let output = cmd.output().await?;

    if !output.status.success() {
        let _ = tokio::fs::remove_file(&output_path).await;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("erreur inconnue");
        return Err(CoreError::YtDlpFailed(format!("ffmpeg : {detail}")));
    }

    Ok(output_path)
}

/// Verifies media container/stream integrity with `ffmpeg -v error -i <file> -f null -`.
/// Returns `Ok(true)` if valid, `Ok(false)` if errors were detected or `ffmpeg` is unavailable.
pub async fn verify_media_integrity(
    paths: &AppPaths,
    settings: &Settings,
    file_path: &Path,
) -> CoreResult<bool> {
    if !file_path.exists() {
        return Ok(false);
    }
    let Ok(ffmpeg_bin) = paths.effective_ffmpeg(settings) else {
        return Ok(true);
    };
    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.args(["-v", "error", "-i"])
        .arg(file_path)
        .args(["-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut cmd);
    let output = cmd.output().await?;
    Ok(output.status.success() && output.stderr.is_empty())
}

/// Builds FFmpeg arguments for a [`vidgrab_shared::PostProcessPipeline`].
#[must_use]
pub fn build_post_process_args(
    input_path: &Path,
    output_path: &Path,
    pipeline: &vidgrab_shared::PostProcessPipeline,
    hw_encoder: &str,
) -> Option<Vec<String>> {
    if !pipeline.normalize_loudness && !pipeline.remove_silence && !pipeline.transcode_hw_h264 {
        return None;
    }
    let mut args = vec![
        "-y".to_string(),
        "-i".to_string(),
        input_path.to_string_lossy().into_owned(),
    ];
    let mut af_filters = Vec::new();
    if pipeline.normalize_loudness {
        af_filters.push("loudnorm=I=-16:TP=-1.5:LRA=11");
    }
    if pipeline.remove_silence {
        af_filters.push("silenceremove=start_periods=1:stop_periods=-1:stop_duration=0.5:stop_threshold=-45dB");
    }
    if !af_filters.is_empty() {
        args.push("-af".to_string());
        args.push(af_filters.join(","));
    }
    if pipeline.transcode_hw_h264 {
        args.push("-c:v".to_string());
        args.push(hw_encoder.to_string());
    } else {
        args.push("-c:v".to_string());
        args.push("copy".to_string());
    }
    args.push(output_path.to_string_lossy().into_owned());
    Some(args)
}

/// Executes [`vidgrab_shared::PostProcessPipeline`] on `input_path` using `ffmpeg`
/// (with auto-detected hardware encoder when `transcode_hw_h264` is enabled).
pub async fn run_post_process_pipeline(
    paths: &AppPaths,
    settings: &Settings,
    input_path: &Path,
) -> CoreResult<PathBuf> {
    let pipe = &settings.post_process_pipeline;
    if !pipe.normalize_loudness && !pipe.remove_silence && !pipe.transcode_hw_h264 {
        return Ok(input_path.to_path_buf());
    }
    let ffmpeg_bin = paths.effective_ffmpeg(settings)?;
    let hw_enc = if pipe.transcode_hw_h264 {
        crate::platform::detect_ffmpeg_hw_encoder(&ffmpeg_bin).await
    } else {
        "libx264"
    };
    let ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let out_path = input_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}_processed.{ext}"));

    let Some(args) = build_post_process_args(input_path, &out_path, pipe, hw_enc) else {
        return Ok(input_path.to_path_buf());
    };
    let mut cmd = Command::new(&ffmpeg_bin);
    cmd.args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut cmd);
    let out = cmd.output().await?;
    if !out.status.success() {
        let _ = tokio::fs::remove_file(&out_path).await;
        return Err(CoreError::YtDlpFailed(format!(
            "Post-process pipeline failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidgrab_shared::PostProcessPipeline;

    #[test]
    fn test_build_post_process_args() {
        let empty = PostProcessPipeline::default();
        assert!(build_post_process_args(Path::new("a.mp4"), Path::new("b.mp4"), &empty, "libx264").is_none());

        let pipe = PostProcessPipeline {
            normalize_loudness: true,
            remove_silence: true,
            transcode_hw_h264: true,
        };
        let args = build_post_process_args(Path::new("a.mp4"), Path::new("b.mp4"), &pipe, "h264_nvenc")
            .expect("args");
        assert!(args.contains(&"h264_nvenc".to_string()));
        assert!(args.iter().any(|a| a.contains("loudnorm") && a.contains("silenceremove")));
    }
}
