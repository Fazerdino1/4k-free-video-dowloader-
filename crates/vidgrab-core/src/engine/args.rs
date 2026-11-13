//! Pure argument construction for `yt-dlp`.
//!
//! Never invokes a shell (`sh -c`). All options are constructed as a strongly-typed
//! `Vec<String>` passed directly to `tokio::process::Command::args`.

use std::path::Path;
use vidgrab_shared::{
    CookieBrowser, DownloadRequest, DubbedTrackMode, DuplicatePolicy, IpVersion, LiveRecordMode,
    MediaType, Settings,
};

/// Prefix printed by yt-dlp before the final output path (`--print after_move:`).
pub const FINAL_PATH_PREFIX: &str = "VGFINAL:";

/// Custom JSON progress template emitted by `yt-dlp` on stdout with `--newline`.
pub const PROGRESS_TEMPLATE: &str = concat!(
    "download:{\"status\":\"%(progress.status)s\",",
    "\"downloaded_bytes\":%(progress.downloaded_bytes|0)s,",
    "\"total_bytes\":%(progress.total_bytes|0)s,",
    "\"total_bytes_estimate\":%(progress.total_bytes_estimate|0)s,",
    "\"speed\":%(progress.speed|0)s,",
    "\"eta\":%(progress.eta|0)s,",
    "\"fragment_index\":%(progress.fragment_index|0)s,",
    "\"fragment_count\":%(progress.fragment_count|0)s,",
    "\"filename\":%(progress.filename|\"\")j}"
);

/// Builds the complete `yt-dlp` command-line argument vector for a given [`DownloadRequest`]
/// and [`Settings`].
///
/// This is a pure function with zero side effects and full unit test coverage.
pub fn build_args(req: &DownloadRequest, settings: &Settings) -> Vec<String> {
    let mut args = Vec::with_capacity(48);

    // 1. Progress & output behavior
    args.push("--newline".to_string());
    args.push("--no-warnings".to_string());
    args.push("--progress-template".to_string());
    args.push(PROGRESS_TEMPLATE.to_string());
    args.push("--progress".to_string());
    // Print the final file path after merge/post-processing so the UI can play it
    args.push("--print".to_string());
    args.push(format!("after_move:{FINAL_PATH_PREFIX}%(filepath)s"));
    args.push("--continue".to_string());

    // 2. Duplicate handling
    match settings.duplicate_policy {
        DuplicatePolicy::Skip => args.push("--no-overwrites".to_string()),
        DuplicatePolicy::Rename => args.push("--no-post-overwrites".to_string()),
        DuplicatePolicy::Overwrite => args.push("--force-overwrites".to_string()),
    }

    // 3. Concurrent fragments & rate limiting
    let fragments = settings.concurrent_fragments.max(1);
    args.push("--concurrent-fragments".to_string());
    args.push(fragments.to_string());

    if settings.speed_limit_kbps > 0 {
        args.push("--limit-rate".to_string());
        args.push(format!("{}K", settings.speed_limit_kbps));
    }

    // 4. Media type, format selection, container & codecs
    match req.media_type {
        MediaType::Video => {
            let format_expr = req
                .explicit_format_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| build_video_format_selector(req));
            args.push("-f".to_string());
            args.push(format_expr);

            args.push("--merge-output-format".to_string());
            args.push(req.video_container.as_str().to_string());

            if req.dubbed_track_mode == DubbedTrackMode::MultiTrackMkv
                && req.audio_languages.len() > 1
            {
                args.push("--audio-multistreams".to_string());
            }
        }
        MediaType::AudioOnly => {
            if let Some(expr) = req
                .explicit_format_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                args.push("-f".to_string());
                args.push(expr.to_string());
            }
            args.push("-x".to_string());
            args.push("--audio-format".to_string());
            args.push(req.audio_format.as_str().to_string());
            args.push("--audio-quality".to_string());
            args.push(req.audio_bitrate.as_yt_dlp_quality().to_string());
        }
        MediaType::SubtitlesOnly => {
            args.push("--skip-download".to_string());
        }
    }

    // 5. Subtitles
    if req.media_type == MediaType::SubtitlesOnly || !req.subtitle_languages.is_empty() {
        args.push("--write-subs".to_string());
        if req.include_auto_subtitles {
            args.push("--write-auto-subs".to_string());
        }
        let langs = if req.subtitle_languages.is_empty() {
            "all".to_string()
        } else {
            req.subtitle_languages.join(",")
        };
        args.push("--sub-langs".to_string());
        args.push(langs);

        args.push("--convert-subs".to_string());
        args.push(req.subtitle_format.as_str().to_string());

        if req.embed_subtitles && req.media_type == MediaType::Video {
            args.push("--embed-subs".to_string());
        }
    }

    // 6. Metadata, thumbnails, chapters & descriptions
    if settings.embed_thumbnail && req.media_type != MediaType::SubtitlesOnly {
        args.push("--embed-thumbnail".to_string());
    }
    if settings.embed_metadata && req.media_type != MediaType::SubtitlesOnly {
        args.push("--embed-metadata".to_string());
    }
    if settings.embed_chapters && req.media_type == MediaType::Video {
        args.push("--embed-chapters".to_string());
    }
    if settings.split_chapters && req.media_type != MediaType::SubtitlesOnly {
        args.push("--split-chapters".to_string());
    }
    if settings.write_description {
        args.push("--write-description".to_string());
    }

    // 7. Live streams
    match req.live_record_mode {
        LiveRecordMode::FromStart => args.push("--live-from-start".to_string()),
        LiveRecordMode::FromNow => args.push("--no-live-from-start".to_string()),
    }

    // 7b. Time section clipping (--download-sections)
    if let Some(section_expr) = crate::media_tools::build_download_section_expr(
        req.clip_start.as_deref(),
        req.clip_end.as_deref(),
    ) {
        args.push("--download-sections".to_string());
        args.push(section_expr);
        args.push("--force-keyframes-at-cuts".to_string());
    }

    // 8. SponsorBlock
    if req.sponsorblock_remove || settings.sponsorblock_enabled {
        args.push("--sponsorblock-remove".to_string());
        args.push("sponsor,selfpromo,interaction,intro,outro".to_string());
    }

    // 9. Network, Proxy, IPv4/IPv6, Geo-bypass & Cookies
    append_network_args(&mut args, &req.url, settings);

    // 10. Custom ffmpeg path if set
    if let Some(ffmpeg_path) = &settings.custom_ffmpeg_path {
        if !ffmpeg_path.as_os_str().is_empty() {
            args.push("--ffmpeg-location".to_string());
            args.push(ffmpeg_path.to_string_lossy().into_owned());
        }
    }

    // 11. Output directory & filename template
    let mut target_dir = req.output_dir.clone();
    if let Some(subfolder) = &req.subfolder_name {
        let sanitized = sanitize_folder_name(subfolder);
        if !sanitized.is_empty() {
            target_dir = target_dir.join(sanitized);
        }
    }

    let template = if req.number_playlist_files {
        if let Some(idx) = req.playlist_index {
            format!("{idx:03} - {}", req.filename_template)
        } else {
            format!("%(playlist_index)03d - {}", req.filename_template)
        }
    } else {
        req.filename_template.clone()
    };

    let full_output: std::path::PathBuf = target_dir.join(template);
    args.push("-o".to_string());
    args.push(full_output.to_string_lossy().into_owned());

    // 12. Validated custom user arguments
    for custom in &settings.custom_yt_dlp_args {
        let trimmed = custom.trim();
        if is_safe_custom_arg(trimmed) {
            args.push(trimmed.to_string());
        }
    }

    // 13. Separator and target URL
    args.push("--".to_string());
    args.push(req.url.clone());

    args
}

/// Builds arguments for `yt-dlp --dump-single-json` analysis.
pub fn build_analyze_args(url: &str, flat_playlist: bool, settings: &Settings) -> Vec<String> {
    let mut args = vec![
        "--dump-single-json".to_string(),
        "--no-warnings".to_string(),
    ];
    if flat_playlist {
        args.push("--flat-playlist".to_string());
    }
    append_network_args(&mut args, url, settings);
    args.push("--".to_string());
    args.push(url.to_string());
    args
}

/// Builds arguments for subscription archive checks (`--download-archive`).
pub fn build_subscription_archive_args(archive_file: &Path, max_downloads: u32) -> Vec<String> {
    let mut args = vec![
        "--download-archive".to_string(),
        archive_file.to_string_lossy().into_owned(),
    ];
    if max_downloads > 0 {
        args.push("--playlist-end".to_string());
        args.push(max_downloads.to_string());
    }
    args
}

fn build_video_format_selector(req: &DownloadRequest) -> String {
    let mut video_filters = Vec::new();
    if let Some(h) = req.target_height {
        video_filters.push(format!("[height<={h}]"));
    }
    if let Some(codec) = req.video_codec.yt_dlp_filter() {
        video_filters.push(format!("[vcodec^={codec}]"));
    }
    if req.prefer_60fps {
        video_filters.push("[fps<=60]".to_string());
    }
    if req.prefer_hdr {
        video_filters.push("[dynamic_range!=SDR]".to_string());
    }

    let v_filter_str = video_filters.join("");
    let fallback_height = req
        .target_height
        .map(|h| format!("[height<={h}]"))
        .unwrap_or_default();

    let audio_selector = if req.audio_languages.is_empty() {
        "bestaudio".to_string()
    } else if req.dubbed_track_mode == DubbedTrackMode::MultiTrackMkv
        && req.audio_languages.len() > 1
    {
        req.audio_languages
            .iter()
            .map(|lang| format!("bestaudio[language={lang}]"))
            .collect::<Vec<_>>()
            .join("+")
    } else {
        let first_lang = &req.audio_languages[0];
        format!("bestaudio[language={first_lang}]/bestaudio")
    };

    if v_filter_str == fallback_height {
        format!("bestvideo{fallback_height}+{audio_selector}/best{fallback_height}/best")
    } else {
        format!(
            "bestvideo{v_filter_str}+{audio_selector}/bestvideo{fallback_height}+bestaudio/best{fallback_height}/best"
        )
    }
}

fn append_network_args(args: &mut Vec<String>, target_url: &str, settings: &Settings) {
    if settings.proxy.enabled && !settings.proxy.url.trim().is_empty() {
        let should_use_proxy = if settings.proxy.only_domains.is_empty() {
            true
        } else if let Ok(parsed) = url::Url::parse(target_url) {
            let host = parsed.host_str().unwrap_or_default().to_lowercase();
            settings.proxy.only_domains.iter().any(|d| {
                let dom = d.trim().to_lowercase();
                !dom.is_empty() && (host == dom || host.ends_with(&format!(".{dom}")))
            })
        } else {
            false
        };

        if should_use_proxy {
            args.push("--proxy".to_string());
            args.push(settings.proxy.url.trim().to_string());
        }
    }

    if let Some(src_ip) = &settings.source_address {
        let trimmed = src_ip.trim();
        if !trimmed.is_empty() {
            args.push("--source-address".to_string());
            args.push(trimmed.to_string());
        }
    }

    match settings.ip_version {
        IpVersion::Auto => {}
        IpVersion::Ipv4Only => args.push("-4".to_string()),
        IpVersion::Ipv6Only => args.push("-6".to_string()),
    }

    if let Some(country) = &settings.geo_bypass_country {
        let trimmed = country.trim();
        if !trimmed.is_empty() {
            args.push("--xff".to_string());
            args.push(trimmed.to_string());
        }
    }

    if let Some(ua) = &settings.user_agent {
        let trimmed = ua.trim();
        if !trimmed.is_empty() {
            args.push("--user-agent".to_string());
            args.push(trimmed.to_string());
        }
    }

    if let Some(cookies_path) = &settings.custom_cookies_path {
        if !cookies_path.as_os_str().is_empty() {
            args.push("--cookies".to_string());
            args.push(cookies_path.to_string_lossy().into_owned());
        }
    } else if let Some(browser) = settings.cookie_browser.yt_dlp_name() {
        args.push("--cookies-from-browser".to_string());
        args.push(browser.to_string());
    } else if settings.cookie_browser == CookieBrowser::InternalWebview {
        if let Some(data_dir) = dirs::data_local_dir() {
            let cookie_file = data_dir.join("vidgrab").join("webview_cookies.txt");
            if cookie_file.exists() {
                args.push("--cookies".to_string());
                args.push(cookie_file.to_string_lossy().into_owned());
            }
        }
    }
}

const MAX_PATH_SEGMENT_CHARS: usize = 120;

/// Sanitizes a single folder or file stem segment for cross-platform safety
/// (strips reserved/control characters, trailing dots/spaces, Windows device names,
/// and caps length to protect against `MAX_PATH` overflows).
pub fn sanitize_folder_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            _ => c,
        })
        .collect();

    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();

    if cleaned.chars().count() > MAX_PATH_SEGMENT_CHARS {
        cleaned = cleaned.chars().take(MAX_PATH_SEGMENT_CHARS).collect();
        cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    }

    let upper = cleaned.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let is_windows_reserved = matches!(
        base,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if is_windows_reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn is_safe_custom_arg(arg: &str) -> bool {
    if arg.is_empty() {
        return false;
    }
    // Disallow dangerous execution flags like --exec
    let forbidden = ["--exec", "--command"];
    for f in forbidden {
        if arg.starts_with(f) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_folder_name_cross_platform() {
        assert_eq!(sanitize_folder_name("  My:Video*Name? . "), "My_Video_Name_");
        assert_eq!(sanitize_folder_name("CON"), "_CON");
        assert_eq!(sanitize_folder_name("nul.txt"), "_nul.txt");
        let long = "a".repeat(200);
        assert_eq!(sanitize_folder_name(&long).chars().count(), MAX_PATH_SEGMENT_CHARS);
    }
}
