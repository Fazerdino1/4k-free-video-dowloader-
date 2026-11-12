//! Parsing of `yt-dlp` JSON metadata and `--progress-template` stdout lines.

use crate::error::CoreResult;
use serde::Deserialize;
use std::path::PathBuf;
use uuid::Uuid;
use vidgrab_shared::{DownloadProgress, DownloadStatus, ThumbnailCandidate, VideoInfo};

/// Raw progress line emitted by `yt-dlp` via `PROGRESS_TEMPLATE`.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawProgressLine {
    status: String,
    downloaded_bytes: Option<f64>,
    total_bytes: Option<f64>,
    total_bytes_estimate: Option<f64>,
    speed: Option<f64>,
    eta: Option<f64>,
    fragment_index: Option<u32>,
    fragment_count: Option<u32>,
    filename: Option<String>,
}

/// Picks the best usable thumbnail URL from `explicit`, `thumbnails` array, or YouTube fallback.
fn resolve_best_thumbnail(
    explicit: Option<String>,
    candidates: &[ThumbnailCandidate],
    id_hint: &str,
    url_hint: &str,
) -> Option<String> {
    if let Some(url) = explicit
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Some(url);
    }
    let best_candidate = candidates
        .iter()
        .filter(|c| c.url.starts_with("http://") || c.url.starts_with("https://"))
        .max_by_key(|c| {
            let w = c.width.unwrap_or(0);
            let h = c.height.unwrap_or(0);
            let in_sweet_spot = u32::from((320..=1280).contains(&w));
            let pref = c.preference.unwrap_or(0).clamp(-100, 100) + 100;
            (in_sweet_spot, pref as u32, w.min(1280), h)
        })
        .map(|c| c.url.clone());
    best_candidate.or_else(|| crate::youtube_thumbnail_fallback(id_hint, url_hint))
}

fn enrich_video_info_thumbnails(info: &mut VideoInfo) {
    let top_url = info.webpage_url.clone().unwrap_or_default();
    for entry in &mut info.entries {
        let entry_url = entry.effective_url();
        entry.thumbnail = resolve_best_thumbnail(
            entry.thumbnail.take(),
            &entry.thumbnails,
            &entry.id,
            &entry_url,
        );
    }
    info.thumbnail =
        resolve_best_thumbnail(info.thumbnail.take(), &info.thumbnails, &info.id, &top_url)
            .or_else(|| info.entries.first().and_then(|e| e.thumbnail.clone()));
}

/// Parses the JSON output of `yt-dlp --dump-single-json` into a strongly-typed [`VideoInfo`].
/// Sometimes `yt-dlp` outputs multiple JSON lines (or warnings before JSON); we parse the last
/// non-empty JSON object line and tolerate any `null` fields.
pub fn parse_video_info(json_str: &str) -> CoreResult<VideoInfo> {
    let trimmed = json_str.trim();
    if let Ok(mut info) = serde_json::from_str::<VideoInfo>(trimmed) {
        enrich_video_info_thumbnails(&mut info);
        return Ok(info);
    }

    // Fallback: parse as generic serde_json::Value and extract fields resiliently
    let val: serde_json::Value = serde_json::from_str(trimmed)?;
    let id = val
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let title = val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Video")
        .to_string();
    let uploader = val
        .get("uploader")
        .and_then(|v| v.as_str())
        .map(String::from);
    let duration = val.get("duration").and_then(|v| v.as_f64());
    let thumbnails: Vec<ThumbnailCandidate> = val
        .get("thumbnails")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let webpage_url = val
        .get("webpage_url")
        .and_then(|v| v.as_str())
        .map(String::from);
    let thumbnail = resolve_best_thumbnail(
        val.get("thumbnail")
            .and_then(|v| v.as_str())
            .map(String::from),
        &thumbnails,
        &id,
        webpage_url.as_deref().unwrap_or(""),
    );

    Ok(VideoInfo {
        id,
        title,
        uploader,
        duration,
        thumbnail,
        thumbnails,
        webpage_url,
        ..Default::default()
    })
}

/// Parses a single stdout line from `yt-dlp` into a [`DownloadProgress`] update if it matches
/// our custom `download:{...}` JSON template.
pub fn parse_progress_line(id: Uuid, line: &str) -> Option<DownloadProgress> {
    let trimmed = line.trim();
    let json_part = trimmed.strip_prefix("download:")?;

    // yt-dlp replaces missing numeric fields with "NA" if not guarded, or 0 with our |0 filter.
    // Sanitize any bare `:NA` occurrences just in case.
    let sanitized = json_part
        .replace(":NA,", ":null,")
        .replace(":NA}", ":null}")
        .replace(":\"NA\"", ":null");

    let raw: RawProgressLine = serde_json::from_str(&sanitized).ok()?;

    let downloaded = raw.downloaded_bytes.unwrap_or(0.0).max(0.0) as u64;
    let total = raw
        .total_bytes
        .filter(|&v| v > 0.0)
        .or_else(|| raw.total_bytes_estimate.filter(|&v| v > 0.0))
        .map(|v| v as u64);

    let percent = match total {
        Some(t) if t > 0 => ((downloaded as f64 / t as f64) * 100.0).clamp(0.0, 100.0) as f32,
        _ => {
            if raw.status == "finished" {
                100.0
            } else {
                0.0
            }
        }
    };

    let status = match raw.status.as_str() {
        "downloading" => DownloadStatus::Downloading,
        "finished" => DownloadStatus::Processing,
        "error" => DownloadStatus::Failed,
        _ => DownloadStatus::Downloading,
    };

    let speed_bps = raw.speed.filter(|&s| s > 0.0);
    let eta_seconds = raw.eta.filter(|&e| e > 0.0).map(|e| e.round() as u64);
    let output_path = raw.filename.filter(|f| !f.is_empty()).map(PathBuf::from);

    Some(DownloadProgress {
        id,
        status,
        downloaded_bytes: downloaded,
        total_bytes: total,
        speed_bps,
        eta_seconds,
        percent,
        fragment_index: raw.fragment_index.filter(|&i| i > 0),
        fragment_count: raw.fragment_count.filter(|&c| c > 0),
        output_path,
        error_message: None,
        retry_count: 0,
        max_retries: 0,
    })
}

/// Generates an `.m3u` playlist file from a `VideoInfo` playlist.
pub fn generate_m3u_content(info: &VideoInfo) -> String {
    let mut out = String::from("#EXTM3U\n");
    if !info.title.is_empty() {
        out.push_str(&format!("#PLAYLIST:{}\n", info.title));
    }
    for entry in &info.entries {
        let dur = entry.duration.map(|d| d.round() as i64).unwrap_or(-1);
        out.push_str(&format!("#EXTINF:{dur},{}\n", entry.title));
        out.push_str(&format!("{}\n", entry.effective_url()));
    }
    out
}

/// Generates a local `.m3u` playlist file pointing to downloaded media files on disk.
pub fn generate_local_m3u_content(title: &str, entries: &[(Option<String>, PathBuf)]) -> String {
    let mut out = String::from("#EXTM3U\n");
    if !title.is_empty() {
        out.push_str(&format!("#PLAYLIST:{title}\n"));
    }
    for (label, path) in entries {
        let name = label
            .as_deref()
            .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("Media"));
        out.push_str(&format!("#EXTINF:-1,{name}\n"));
        out.push_str(&format!("{}\n", path.display()));
    }
    out
}

/// Converts raw `.srt` / `.vtt` / `.ass` subtitle files into deduplicated plain text paragraphs.
pub fn clean_subtitles_to_txt(raw: &str) -> String {
    let mut result: Vec<String> = Vec::new();
    let mut last_line = String::new();
    let mut in_header = false;

    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            in_header = false;
            continue;
        }
        if line == "WEBVTT"
            || line.starts_with("Kind:")
            || line.starts_with("Language:")
            || line.starts_with("STYLE")
            || line.starts_with("NOTE")
            || line.starts_with("[Script Info]")
            || line.starts_with("[V4+ Styles]")
            || line.starts_with("[Events]")
            || line.starts_with("Format:")
            || line.starts_with("Style:")
        {
            if line.starts_with("STYLE") || line.starts_with("NOTE") {
                in_header = true;
            }
            continue;
        }
        if in_header {
            continue;
        }
        // Skip SRT numeric cue indices and timestamp ranges (`00:00:01.000 --> 00:00:04.000`)
        if line.chars().all(|c| c.is_ascii_digit()) || line.contains("-->") {
            continue;
        }

        // Handle ASS `Dialogue: ...,Text`
        let text_slice = if let Some(rest) = line.strip_prefix("Dialogue:") {
            rest.splitn(10, ',').nth(9).unwrap_or("")
        } else {
            line
        };

        let cleaned = strip_subtitle_markup(text_slice);
        if cleaned.is_empty() || cleaned == last_line {
            continue;
        }
        last_line = cleaned.clone();
        result.push(cleaned);
    }

    result.join("\n")
}

fn strip_subtitle_markup(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_angle = false;
    let mut in_brace = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => in_angle = true,
            '>' if in_angle => in_angle = false,
            '{' => in_brace = true,
            '}' if in_brace => in_brace = false,
            '\\' if !in_angle && !in_brace => {
                if matches!(chars.peek(), Some('N' | 'n')) {
                    chars.next();
                    out.push(' ');
                } else {
                    out.push(ch);
                }
            }
            _ if !in_angle && !in_brace => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
