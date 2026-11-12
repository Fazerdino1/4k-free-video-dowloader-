//! Reusable UI components (Download Card, Toolbar, Modal Dialogs, Status Bar).

use crate::i18n::I18nCatalog;
use vidgrab_shared::{DownloadItem, DownloadStatus, MediaType};

/// Formats a byte count into a human-readable string (`MiB` / `GiB`).
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Formats a speed in bytes/sec into `MiB/s` or `KiB/s`.
pub fn format_speed(bps: f64) -> String {
    format!("{}/s", format_bytes(bps.max(0.0) as u64))
}

/// Renders a single [`DownloadItem`] card as accessible semantic HTML matching our dark studio layout.
pub fn render_download_card(item: &DownloadItem, i18n: &I18nCatalog) -> String {
    let id = item.request.id;
    let title = html_escape(&item.request.title);
    let status_label = match item.progress.status {
        DownloadStatus::Queued => i18n.t("status-queued"),
        DownloadStatus::Downloading => i18n.t("status-downloading"),
        DownloadStatus::Paused => i18n.t("status-paused"),
        DownloadStatus::Processing => i18n.t("status-processing"),
        DownloadStatus::Completed => i18n.t("status-completed"),
        DownloadStatus::Failed => i18n.t("status-failed"),
        DownloadStatus::Cancelled => i18n.t("status-cancelled"),
    };

    let format_tag = match item.request.media_type {
        MediaType::Video => item.request.video_container.as_str().to_uppercase(),
        MediaType::AudioOnly => item.request.audio_format.extension().to_uppercase(),
        MediaType::SubtitlesOnly => item.request.subtitle_format.as_str().to_uppercase(),
    };

    let quality_tag = match item.request.media_type {
        MediaType::Video => format!("{}p", item.request.target_height.unwrap_or(1080)),
        MediaType::AudioOnly => format!("{} kbps", item.request.audio_bitrate.kbps()),
        MediaType::SubtitlesOnly => String::from("Subtitles"),
    };

    let size_info = match item.progress.total_bytes {
        Some(tot) if tot > 0 => format!(
            "{} / {}",
            format_bytes(item.progress.downloaded_bytes),
            format_bytes(tot)
        ),
        _ if item.progress.downloaded_bytes > 0 => format_bytes(item.progress.downloaded_bytes),
        _ => String::from("—"),
    };

    let speed_info = item
        .progress
        .speed_bps
        .map(format_speed)
        .unwrap_or_default();

    let thumb_html = match &item.request.thumbnail {
        Some(url)
            if url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("data:image/") =>
        {
            format!(
                r#"<img src="{}" alt="Thumbnail" loading="lazy" referrerpolicy="no-referrer" /><span class="thumb-play-overlay">▶</span>"#,
                html_escape(url)
            )
        }
        _ => String::from(r#"<span class="thumb-play-overlay">▶</span>"#),
    };

    let category = match item.request.media_type {
        MediaType::Video => "video",
        MediaType::AudioOnly => "audio",
        MediaType::SubtitlesOnly => "subtitles",
    };

    let out_dir = html_escape(&item.request.output_dir.to_string_lossy());
    let file_path = item
        .progress
        .output_path
        .as_ref()
        .map(|p| html_escape(&p.to_string_lossy()))
        .unwrap_or_default();

    format!(
        r#"<article class="download-card" role="listitem" data-id="{id}" data-category="{category}" data-out="{out_dir}" data-file="{file_path}" tabindex="0">
            <div class="thumb-box" data-action="play" data-id="{id}" data-file="{file_path}" data-out="{out_dir}" style="cursor:pointer;" title="Lire le média">{thumb_html}</div>
            <div class="item-meta">
                <div class="item-title" title="{title}">{title}</div>
                <div class="item-sub">
                    <span>{size_info}</span>
                    <span class="dot-sep">·</span>
                    <span>{format_tag}</span>
                    <span class="dot-sep">·</span>
                    <span>{quality_tag}</span>
                    <span class="dot-sep">·</span>
                    <span>{status_label}</span>
                    {speed_span}
                </div>
                <div class="progress-track" role="progressbar" aria-valuenow="{pct:.0}" aria-valuemin="0" aria-valuemax="100">
                    <div class="progress-fill" style="width: {pct:.1}%"></div>
                </div>
            </div>
            <div class="item-actions">
                <button class="row-btn" data-action="pause" data-id="{id}" title="Mettre en pause" aria-label="Pause">⏸</button>
                <button class="row-btn" data-action="resume" data-id="{id}" title="Reprendre / Lire" aria-label="Resume">▶</button>
                <button class="row-btn" data-action="open" data-id="{id}" data-out="{out_dir}" title="Ouvrir le dossier" aria-label="Open folder">📁</button>
                <button class="row-btn" data-action="remove" data-id="{id}" title="Supprimer de la liste" aria-label="Remove">✕</button>
            </div>
        </article>"#,
        speed_span = if speed_info.is_empty() {
            String::new()
        } else {
            format!(r#"<span class="dot-sep">·</span><span>{speed_info}</span>"#)
        },
        pct = item.progress.percent
    )
}

/// Escapes special HTML characters to prevent XSS.
pub fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
