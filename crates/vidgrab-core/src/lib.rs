//! Core business logic for **4k free video dowloader** (`vidgrab-core`).
//!
//! Completely decoupled from Tauri so it can be unit-tested and integration-tested
//! in isolation. Includes the `yt-dlp` engine, SQLite database, download queue with
//! semaphore concurrency, subscriptions, Smart Mode, Netscape cookies, AI audio
//! processing (Demucs + `ffmpeg`), proxy testing, and bulk URL import.

pub mod ai_audio;
pub mod config;
pub mod cookies;
pub mod db;
pub mod engine;
pub mod error;
pub mod queue;
pub mod smart_mode;
pub mod subscriptions;
pub mod updater;

pub mod cli;
pub mod media_tools;
pub mod mobile_bridge;
pub mod platform;

use crate::error::{CoreError, CoreResult};
use base64::{engine::general_purpose::STANDARD as BASE64_STD, Engine as _};
use std::sync::OnceLock;
use std::time::Duration;

static SHARED_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Returns a process-wide pooled HTTP client reused across thumbnail fetches and self-updates.
#[must_use]
pub fn shared_http_client() -> &'static reqwest::Client {
    SHARED_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(8)
            .user_agent(concat!(
                "4k-free-video-dowloader/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Downloads a remote thumbnail image and converts it into a `data:image/...;base64,...` URI
/// so it is cached locally in SQLite and renders offline without CSP/CORS issues.
pub async fn fetch_thumbnail_data_uri(thumb_url: &str) -> Option<String> {
    if thumb_url.starts_with("data:image/") {
        return Some(thumb_url.to_string());
    }
    if !thumb_url.starts_with("http://") && !thumb_url.starts_with("https://") {
        return None;
    }
    let client = shared_http_client();
    let resp = client
        .get(thumb_url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let header_mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(str::trim)
        .filter(|m| m.starts_with("image/"))
        .map(str::to_string);
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let mime = header_mime.unwrap_or_else(|| sniff_image_mime(&bytes).to_string());
    let b64 = BASE64_STD.encode(&bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

fn is_valid_yt_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Extracts a fallback YouTube `hqdefault.jpg` thumbnail URL when an entry has a YouTube ID or URL.
#[must_use]
pub fn youtube_thumbnail_fallback(id_hint: &str, url_hint: &str) -> Option<String> {
    let trimmed_id = id_hint.trim();
    if is_valid_yt_id(trimmed_id)
        && (url_hint.is_empty()
            || url_hint.contains("youtube.com")
            || url_hint.contains("youtu.be"))
    {
        return Some(format!("https://i.ytimg.com/vi/{trimmed_id}/hqdefault.jpg"));
    }
    if let Ok(parsed) = reqwest::Url::parse(url_hint) {
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        if host.contains("youtu.be") {
            if let Some(seg) = parsed
                .path_segments()
                .and_then(|mut s| s.next())
                .filter(|s| is_valid_yt_id(s))
            {
                return Some(format!("https://i.ytimg.com/vi/{seg}/hqdefault.jpg"));
            }
        } else if host.contains("youtube.com") {
            if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "v") {
                if is_valid_yt_id(&v) {
                    return Some(format!("https://i.ytimg.com/vi/{v}/hqdefault.jpg"));
                }
            }
            let segs: Vec<&str> = parsed.path_segments().into_iter().flatten().collect();
            if segs.len() >= 2
                && matches!(segs[0], "shorts" | "embed" | "live" | "v")
                && is_valid_yt_id(segs[1])
            {
                return Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", segs[1]));
            }
        }
    }
    None
}

/// Downloads a remote thumbnail into a `data:image/...` URI, falling back to `hqdefault.jpg` on YouTube.
pub async fn fetch_thumbnail_with_fallback(
    primary_url: Option<&str>,
    id_hint: &str,
    url_hint: &str,
) -> Option<String> {
    if let Some(url) = primary_url.map(str::trim).filter(|u| !u.is_empty()) {
        if let Some(data) = fetch_thumbnail_data_uri(url).await {
            return Some(data);
        }
    }
    if let Some(fallback_url) = youtube_thumbnail_fallback(id_hint, url_hint) {
        if primary_url.map(str::trim) != Some(fallback_url.as_str()) {
            return fetch_thumbnail_data_uri(&fallback_url).await;
        }
    }
    None
}

/// Parses a `.txt` or `.csv` file content into a deduplicated list of valid HTTP/HTTPS URLs
/// or `ytsearch` queries for bulk downloading.
pub fn parse_bulk_urls(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Handle CSV rows by inspecting comma/semicolon-separated columns
        for col in trimmed.split([',', ';', '\t']) {
            let token = col.trim().trim_matches('"').trim_matches('\'');
            if (token.starts_with("http://")
                || token.starts_with("https://")
                || token.starts_with("ytsearch"))
                && !urls.contains(&token.to_string())
            {
                urls.push(token.to_string());
            }
        }
    }
    urls
}

/// Tests connectivity through a user-configured HTTP/HTTPS/SOCKS5 proxy using `reqwest`.
pub async fn test_proxy_connection(proxy_url: &str, test_target_url: &str) -> CoreResult<u128> {
    let proxy = reqwest::Proxy::all(proxy_url)
        .map_err(|e| CoreError::Config(format!("Invalid proxy URL '{proxy_url}': {e}")))?;

    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(10))
        .user_agent(concat!(
            "4k-free-video-dowloader/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?;

    let start = std::time::Instant::now();
    let resp = client.get(test_target_url).send().await?;
    let _ = resp.error_for_status()?;
    Ok(start.elapsed().as_millis())
}

/// Returns `true` when downloads are permitted at `local_hour` (`0..=23`) given `settings`.
pub fn is_within_schedule(settings: &vidgrab_shared::Settings, local_hour: u8) -> bool {
    if !settings.schedule_enabled {
        return true;
    }
    let start = settings.schedule_start_hour % 24;
    let end = settings.schedule_end_hour % 24;
    let hour = local_hour % 24;
    if start == end {
        return true;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

/// Applies `CREATE_NO_WINDOW` on Windows so background subprocesses (`yt-dlp.exe`,
/// `ffmpeg.exe`, `taskkill`, PowerShell) do not flash console windows in GUI mode.
pub fn configure_background_command(cmd: &mut tokio::process::Command) {
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

/// Returns `(available_bytes, total_bytes)` on the filesystem containing `path` (or its closest
/// existing parent directory) directly via OS APIs (`statvfs` on Unix, `GetDiskFreeSpaceExW` on Windows).
pub fn get_disk_space_bytes(path: &std::path::Path) -> Option<(u64, u64)> {
    let mut target = if path.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        path.to_path_buf()
    };
    while !target.exists() {
        match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => target = p.to_path_buf(),
            _ => {
                target = std::path::PathBuf::from(".");
                break;
            }
        }
    }
    #[cfg(unix)]
    {
        let stats = nix::sys::statvfs::statvfs(&target).ok()?;
        let unit = if stats.fragment_size() > 0 {
            stats.fragment_size() as u64
        } else {
            stats.block_size() as u64
        };
        let avail = (stats.blocks_available() as u64).saturating_mul(unit);
        let total = (stats.blocks() as u64).saturating_mul(unit);
        Some((avail, total))
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = target
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut avail: u64 = 0;
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        // SAFETY: `wide` is NUL-terminated UTF-16 and output pointers are valid stack locals.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
        if ok != 0 {
            Some((avail, total))
        } else {
            None
        }
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = target;
        None
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::config::{load_or_create_settings, save_settings, AppPaths};
    use crate::cookies::{
        format_netscape_cookies, parse_netscape_cookies, write_netscape_cookies_file,
    };
    use crate::db::Database;
    use crate::engine::args::build_args;
    use crate::engine::parser::{
        clean_subtitles_to_txt, generate_local_m3u_content, generate_m3u_content,
        parse_progress_line, parse_video_info,
    };
    use crate::smart_mode::{build_smart_download_request, resolve_smart_height};
    use crate::subscriptions::SubscriptionManager;
    use crate::updater::{compute_sha256_hex, parse_sha256sums, verify_sha256};
    use chrono::Utc;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, RwLock};
    use tokio::sync::mpsc;
    use uuid::Uuid;
    use vidgrab_shared::*;

    #[test]
    fn test_build_args_video_4k_mkv_vp9_hdr_60fps_multitrack() {
        let req = DownloadRequest {
            id: Uuid::nil(),
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
            title: "Test 4K Video".to_string(),
            media_type: MediaType::Video,
            target_height: Some(2160),
            prefer_60fps: true,
            prefer_hdr: true,
            video_container: VideoContainer::Mkv,
            video_codec: VideoCodec::Vp9,
            audio_languages: vec!["fr".to_string(), "en".to_string()],
            dubbed_track_mode: DubbedTrackMode::MultiTrackMkv,
            subtitle_languages: vec!["fr".to_string(), "en".to_string()],
            include_auto_subtitles: true,
            subtitle_format: SubtitleFormat::Srt,
            embed_subtitles: true,
            output_dir: PathBuf::from("/tmp/videos"),
            filename_template: "%(title)s.%(ext)s".to_string(),
            subfolder_name: Some("My Channel: Best/Worst".to_string()),
            number_playlist_files: true,
            playlist_index: Some(7),
            live_record_mode: LiveRecordMode::FromStart,
            sponsorblock_remove: true,
            ..Default::default()
        };

        let mut settings = Settings::default();
        settings.speed_limit_kbps = 2048;
        settings.concurrent_fragments = 8;
        settings.proxy.enabled = true;
        settings.proxy.url = "socks5://127.0.0.1:1080".to_string();
        settings.ip_version = IpVersion::Ipv4Only;
        settings.geo_bypass_country = Some("FR".to_string());
        settings.source_address = Some("192.168.1.50".to_string());

        let args = build_args(&req, &settings);

        assert!(args.contains(&"--newline".to_string()));
        assert!(args.contains(&"--continue".to_string()));
        assert!(args.contains(&"--merge-output-format".to_string()));
        assert!(args.contains(&"mkv".to_string()));
        assert!(args.contains(&"--audio-multistreams".to_string()));
        assert!(args.contains(&"--write-subs".to_string()));
        assert!(args.contains(&"--write-auto-subs".to_string()));
        assert!(args.contains(&"fr,en".to_string()));
        assert!(args.contains(&"--embed-subs".to_string()));
        assert!(args.contains(&"--live-from-start".to_string()));
        assert!(args.contains(&"--sponsorblock-remove".to_string()));
        assert!(args.contains(&"--limit-rate".to_string()));
        assert!(args.contains(&"2048K".to_string()));
        assert!(args.contains(&"--proxy".to_string()));
        assert!(args.contains(&"socks5://127.0.0.1:1080".to_string()));
        assert!(args.contains(&"-4".to_string()));
        assert!(args.contains(&"--xff".to_string()));
        assert!(args.contains(&"FR".to_string()));
        assert!(args.contains(&"-o".to_string()));
        let expected_out = PathBuf::from("/tmp/videos")
            .join("My Channel_ Best_Worst")
            .join("007 - %(title)s.%(ext)s")
            .to_string_lossy()
            .into_owned();
        assert!(args.contains(&expected_out));
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
    }

    #[test]
    fn test_build_args_audio_only_mp3_320k() {
        let req = DownloadRequest {
            url: "https://soundcloud.com/artist/track".to_string(),
            media_type: MediaType::AudioOnly,
            audio_format: AudioFormat::Mp3,
            audio_bitrate: AudioBitrate::Kbps320,
            output_dir: PathBuf::from("/tmp/music"),
            ..Default::default()
        };
        let settings = Settings::default();
        let args = build_args(&req, &settings);

        assert!(args.contains(&"-x".to_string()));
        assert!(args.contains(&"--audio-format".to_string()));
        assert!(args.contains(&"mp3".to_string()));
        assert!(args.contains(&"--audio-quality".to_string()));
        assert!(args.contains(&"320K".to_string()));
    }

    #[test]
    fn test_build_args_subtitles_only_ass() {
        let req = DownloadRequest {
            url: "https://vimeo.com/123456".to_string(),
            media_type: MediaType::SubtitlesOnly,
            subtitle_languages: vec!["en".to_string()],
            subtitle_format: SubtitleFormat::Ass,
            ..Default::default()
        };
        let settings = Settings::default();
        let args = build_args(&req, &settings);

        assert!(args.contains(&"--skip-download".to_string()));
        assert!(args.contains(&"--write-subs".to_string()));
        assert!(args.contains(&"ass".to_string()));
    }

    #[test]
    fn test_parse_yt_dlp_video_info_real_fixture() {
        let fixture = r#"{
            "id": "aqz-KE-bpKQ",
            "title": "Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film",
            "uploader": "Blender Foundation",
            "duration": 634.5,
            "webpage_url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "extractor_key": "Youtube",
            "unknown_future_field": {"nested": [1, 2, 3]},
            "formats": [
                {
                    "format_id": "137",
                    "ext": "mp4",
                    "width": 1920,
                    "height": 1080,
                    "fps": 60.0,
                    "vcodec": "avc1.64002a",
                    "acodec": "none",
                    "filesize": 154829312,
                    "dynamic_range": "SDR"
                },
                {
                    "format_id": "315",
                    "ext": "webm",
                    "width": 3840,
                    "height": 2160,
                    "fps": 60.0,
                    "vcodec": "vp9",
                    "acodec": "none",
                    "filesize_approx": 612000000,
                    "dynamic_range": "HDR10",
                    "projection": "equirectangular"
                },
                {
                    "format_id": "251-fr",
                    "ext": "webm",
                    "vcodec": "none",
                    "acodec": "opus",
                    "abr": 160.0,
                    "language": "fr"
                },
                {
                    "format_id": "251-en",
                    "ext": "webm",
                    "vcodec": "none",
                    "acodec": "opus",
                    "abr": 160.0,
                    "language": "en"
                }
            ],
            "subtitles": {
                "fr": [{"ext": "vtt", "name": "Français"}],
                "en": [{"ext": "srt", "name": "English"}]
            },
            "chapters": [
                {"title": "Intro", "start_time": 0.0, "end_time": 45.0}
            ]
        }"#;

        let info = parse_video_info(fixture).unwrap_or_else(|e| panic!("parse failed: {e}"));
        assert_eq!(info.id, "aqz-KE-bpKQ");
        assert_eq!(info.available_heights(), vec![2160, 1080]);
        assert_eq!(
            info.available_audio_languages(),
            vec!["en".to_string(), "fr".to_string()]
        );
        assert_eq!(
            info.detected_projection(),
            ProjectionType::Equirectangular360
        );
        assert_eq!(info.formats[1].display_quality(), "4K (2160p) 60fps HDR10");
        assert_eq!(info.formats[1].estimated_size(), Some(612_000_000));
    }

    #[test]
    fn test_parse_progress_line() {
        let id = Uuid::new_v4();
        let line = r#"download:{"status":"downloading","downloaded_bytes":52428800,"total_bytes":104857600,"total_bytes_estimate":0,"speed":10485760.0,"eta":5,"fragment_index":2,"fragment_count":10,"filename":"/tmp/video.mp4"}"#;
        let prog = parse_progress_line(id, line).unwrap_or_else(|| panic!("expected progress"));
        assert_eq!(prog.id, id);
        assert_eq!(prog.status, DownloadStatus::Downloading);
        assert_eq!(prog.downloaded_bytes, 52_428_800);
        assert_eq!(prog.total_bytes, Some(104_857_600));
        assert!((prog.percent - 50.0).abs() < 0.01);
        assert_eq!(prog.eta_seconds, Some(5));
        assert_eq!(prog.fragment_index, Some(2));
        assert_eq!(prog.fragment_count, Some(10));
        assert_eq!(prog.output_path, Some(PathBuf::from("/tmp/video.mp4")));
    }

    #[test]
    fn test_smart_mode_quality_fallback() {
        let available = vec![2160, 1440, 720, 480, 360];
        // Request 1080p when only 2160, 1440, 720 exist -> falls back to 720p (best lower quality)
        assert_eq!(resolve_smart_height(Some(1080), &available), Some(720));
        // Exact match
        assert_eq!(resolve_smart_height(Some(1440), &available), Some(1440));
        // Request lower than minimum -> falls back to lowest available (360p)
        assert_eq!(resolve_smart_height(Some(144), &available), Some(360));

        let mut info = VideoInfo::default();
        info.id = "xyz123".to_string();
        info.title = "Smart Video".to_string();
        info.formats = vec![
            Format {
                format_id: "22".to_string(),
                height: Some(720),
                vcodec: Some("avc1".to_string()),
                ..Default::default()
            },
            Format {
                format_id: "18".to_string(),
                height: Some(360),
                vcodec: Some("avc1".to_string()),
                ..Default::default()
            },
        ];

        let mut settings = Settings::default();
        settings.smart_mode.enabled = true;
        settings.smart_mode.target_height = Some(1080);

        let req = build_smart_download_request(&info, &settings);
        assert_eq!(req.target_height, Some(720));
        assert_eq!(req.title, "Smart Video");
    }

    #[test]
    fn test_netscape_cookies_roundtrip_and_0600_permissions() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let cookie_path = dir.path().join("cookies.txt");

        let cookies = vec![
            CookieEntry {
                domain: ".youtube.com".to_string(),
                include_subdomains: true,
                path: "/".to_string(),
                secure: true,
                expires: 1893456000,
                name: "SID".to_string(),
                value: "secret_token_123".to_string(),
            },
            CookieEntry {
                domain: "vimeo.com".to_string(),
                include_subdomains: false,
                path: "/".to_string(),
                secure: false,
                expires: 1893456000,
                name: "vuid".to_string(),
                value: "vimeo_456".to_string(),
            },
        ];

        write_netscape_cookies_file(&cookie_path, &cookies)
            .unwrap_or_else(|e| panic!("write cookies failed: {e}"));

        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&cookie_path)
                .unwrap_or_else(|e| panic!("meta: {e}"))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let raw = format_netscape_cookies(&cookies);
        let parsed = parse_netscape_cookies(&raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "SID");
        assert_eq!(parsed[0].value, "secret_token_123");
        assert!(parsed[0].secure);
    }

    #[tokio::test]
    async fn test_sqlite_db_queue_subscriptions_and_export_import() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let paths = AppPaths::from_roots(&dir.path().join("cfg"), &dir.path().join("data"))
            .unwrap_or_else(|e| panic!("paths: {e}"));

        let settings = load_or_create_settings(&paths).unwrap_or_else(|e| panic!("settings: {e}"));
        save_settings(&paths, &settings).unwrap_or_else(|e| panic!("save settings: {e}"));

        let db = Arc::new(Database::open(&paths.db_path).unwrap_or_else(|e| panic!("db: {e}")));

        // 1. Insert an interrupted download and verify crash recovery
        let mut req = DownloadRequest::default();
        req.title = "Recoverable Video".to_string();
        req.url = "https://www.youtube.com/watch?v=recover".to_string();
        let item = DownloadItem {
            request: req.clone(),
            progress: DownloadProgress {
                id: req.id,
                status: DownloadStatus::Downloading,
                percent: 42.5,
                ..Default::default()
            },
            created_at: Utc::now(),
            completed_at: None,
        };
        db.upsert_download(&item)
            .unwrap_or_else(|e| panic!("upsert: {e}"));

        let (tx, _rx) = mpsc::unbounded_channel();
        let shared_settings = Arc::new(RwLock::new(settings));
        let queue = crate::queue::DownloadQueue::new(
            paths.clone(),
            db.clone(),
            shared_settings.clone(),
            tx,
        )
        .unwrap_or_else(|e| panic!("queue init: {e}"));

        let items = queue.list_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].progress.status, DownloadStatus::Queued);

        // 2. Subscriptions & archive tracking
        let sub_mgr = SubscriptionManager::new(paths, db.clone(), queue, shared_settings);
        let sub = Subscription {
            id: Uuid::new_v4(),
            url: "https://www.youtube.com/@BlenderFoundation/videos".to_string(),
            title: "Blender Foundation".to_string(),
            thumbnail: None,
            check_interval_mins: 60,
            media_type: MediaType::Video,
            target_height: Some(1080),
            video_container: VideoContainer::Mp4,
            audio_format: AudioFormat::Mp3,
            output_dir: PathBuf::from("/tmp"),
            max_videos: 5,
            include_shorts: false,
            include_lives: false,
            paused: false,
            last_checked_at: None,
            created_at: Utc::now(),
        };
        sub_mgr
            .save_subscription(&sub)
            .unwrap_or_else(|e| panic!("save sub: {e}"));
        sub_mgr
            .record_archived_id(sub.id, "youtube", "vid_001")
            .unwrap_or_else(|e| panic!("record archive: {e}"));
        assert!(sub_mgr.read_archived_ids(sub.id).contains("vid_001"));

        // 3. Export & Import JSON bundle
        let exported_json = db.export_json().unwrap_or_else(|e| panic!("export: {e}"));
        let db2_path = dir.path().join("db2.sqlite3");
        let db2 = Database::open(&db2_path).unwrap_or_else(|e| panic!("db2: {e}"));
        let (dl_count, sub_count) = db2
            .import_json(&exported_json)
            .unwrap_or_else(|e| panic!("import: {e}"));
        assert_eq!(dl_count, 1);
        assert_eq!(sub_count, 1);
    }

    #[test]
    fn test_sha256_verification_and_drm_detection() {
        let data = b"yt-dlp-binary-payload";
        let digest = compute_sha256_hex(data);
        let sums = format!("{digest} *yt-dlp_linux\n0000000000000000000000000000000000000000000000000000000000000000  other_file\n");
        let parsed = parse_sha256sums(&sums, "yt-dlp_linux").unwrap_or_default();
        assert_eq!(parsed, digest);
        assert!(verify_sha256(data, &parsed).is_ok());
        assert!(verify_sha256(b"corrupted", &parsed).is_err());

        // DRM protection check
        assert!(matches!(
            CoreError::check_drm_url("https://www.netflix.com/watch/80057281"),
            Err(CoreError::DrmProtected { .. })
        ));
        assert!(matches!(
            CoreError::check_drm_url("https://www.disneyplus.com/video/123"),
            Err(CoreError::DrmProtected { .. })
        ));
        assert!(CoreError::check_drm_url("https://www.youtube.com/watch?v=aqz-KE-bpKQ").is_ok());
    }

    #[test]
    fn test_bulk_url_parser_and_m3u_generation() {
        let bulk = "# Comment line\nhttps://www.youtube.com/watch?v=111\n\"Title\",\"https://vimeo.com/222\"\nhttps://www.youtube.com/watch?v=111\n";
        let urls = parse_bulk_urls(bulk);
        assert_eq!(
            urls,
            vec![
                "https://www.youtube.com/watch?v=111".to_string(),
                "https://vimeo.com/222".to_string()
            ]
        );

        let mut playlist = VideoInfo::default();
        playlist.title = "My Playlist".to_string();
        playlist.entries.push(PlaylistEntry {
            id: "abc".to_string(),
            title: "Track 1".to_string(),
            duration: Some(120.0),
            webpage_url: Some("https://www.youtube.com/watch?v=abc".to_string()),
            ..Default::default()
        });
        let m3u = generate_m3u_content(&playlist);
        assert!(m3u.contains("#EXTM3U"));
        assert!(m3u.contains("#PLAYLIST:My Playlist"));
        assert!(m3u.contains("#EXTINF:120,Track 1"));

        let local_m3u = generate_local_m3u_content(
            "Local Mix",
            &[(Some("Track A".to_string()), PathBuf::from("/tmp/a.mp4"))],
        );
        assert!(local_m3u.contains("/tmp/a.mp4"));
    }

    #[test]
    fn test_clean_subtitles_schedule_explicit_format_and_disk_space() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: fr\n\n00:00:01.000 --> 00:00:03.000\n<c>Bonjour</c> le monde\n\n00:00:03.000 --> 00:00:05.000\nBonjour le monde\n00:00:05.500 --> 00:00:07.000\nDeuxième ligne";
        let txt = clean_subtitles_to_txt(vtt);
        assert_eq!(txt, "Bonjour le monde\nDeuxième ligne");

        let mut settings = Settings::default();
        settings.schedule_enabled = true;
        settings.schedule_start_hour = 23;
        settings.schedule_end_hour = 6;
        assert!(is_within_schedule(&settings, 0));
        assert!(is_within_schedule(&settings, 23));
        assert!(!is_within_schedule(&settings, 14));

        let mut req = DownloadRequest::default();
        req.explicit_format_id = Some("315+251".to_string());
        let args = build_args(&req, &settings);
        let pos = args
            .iter()
            .position(|a| a == "-f")
            .unwrap_or_else(|| panic!("missing -f"));
        assert_eq!(args[pos + 1], "315+251");

        let space = get_disk_space_bytes(Path::new("."));
        assert!(space.is_some());
        let nested = get_disk_space_bytes(Path::new("./does_not_exist/nested/dir"));
        assert!(matches!(nested, Some((avail, total)) if total > 0 && avail <= total));
    }

    #[test]
    fn test_time_section_clipping_and_download_sections_args() {
        use crate::media_tools::{build_download_section_expr, sanitize_timestamp};

        assert_eq!(sanitize_timestamp(" 01:25 "), Some("01:25".to_string()));
        assert_eq!(
            sanitize_timestamp("01:02:03.250"),
            Some("01:02:03.250".to_string())
        );
        assert_eq!(sanitize_timestamp("01:99"), None);
        assert_eq!(sanitize_timestamp("00:10;rm -rf"), None);

        assert_eq!(
            build_download_section_expr(Some("00:30"), Some("02:15")),
            Some("*00:30-02:15".to_string())
        );
        assert_eq!(
            build_download_section_expr(Some("45"), None),
            Some("*45-inf".to_string())
        );
        assert_eq!(
            build_download_section_expr(None, Some("01:00")),
            Some("*0-01:00".to_string())
        );

        let mut req = DownloadRequest::default();
        req.clip_start = Some("00:15".to_string());
        req.clip_end = Some("01:45".to_string());
        let args = build_args(&req, &Settings::default());
        let pos = args
            .iter()
            .position(|a| a == "--download-sections")
            .unwrap_or_else(|| panic!("missing --download-sections"));
        assert_eq!(args[pos + 1], "*00:15-01:45");
        assert!(args.contains(&"--force-keyframes-at-cuts".to_string()));
    }

    #[test]
    fn test_media_tools_ffmpeg_args_and_transient_errors() {
        use crate::media_tools::{build_ffmpeg_tool_args, build_output_path};

        let src = PathBuf::from("/tmp/sample_video.mkv");
        let dst = build_output_path(&src, &MediaToolAction::RemuxMp4);
        assert_eq!(dst, PathBuf::from("/tmp/sample_video_remux.mp4"));

        let remux_args = build_ffmpeg_tool_args(&src, &dst, &MediaToolAction::RemuxMp4).unwrap();
        assert!(remux_args.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(remux_args
            .windows(2)
            .any(|w| w == ["-movflags", "+faststart"]));

        let mp3_args = build_ffmpeg_tool_args(
            &src,
            Path::new("/tmp/out.mp3"),
            &MediaToolAction::ExtractAudioMp3,
        )
        .unwrap();
        assert!(mp3_args.windows(2).any(|w| w == ["-b:a", "320k"]));

        let bad_trim = build_ffmpeg_tool_args(
            &src,
            Path::new("/tmp/out.mkv"),
            &MediaToolAction::TrimClip {
                start: "invalid".into(),
                end: "".into(),
            },
        );
        assert!(bad_trim.is_err());

        assert!(CoreError::YtDlpFailed("HTTP Error 429: Too Many Requests".into()).is_transient());
        assert!(CoreError::YtDlpFailed("Connection timed out (503)".into()).is_transient());
        assert!(!CoreError::DrmProtected {
            site: "Netflix".into()
        }
        .is_transient());
        assert!(!CoreError::YtDlpFailed("ERROR: Private video".into()).is_transient());
        assert!(!CoreError::YtDlpFailed("Video unavailable (404)".into()).is_transient());
    }

    #[test]
    fn test_db_batch_delete_and_empty_string_binding() {
        let dir = std::env::temp_dir().join(format!("vidgrab_batch_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("test.db")).unwrap();

        let make_item = |title: &str| DownloadItem {
            request: DownloadRequest {
                title: title.to_string(),
                ..Default::default()
            },
            progress: DownloadProgress::default(),
            created_at: Utc::now(),
            completed_at: None,
        };

        let a = make_item("Keep Me");
        let b = make_item("");
        let c = make_item("Delete Two");
        db.upsert_download(&a).unwrap();
        db.upsert_download(&b).unwrap();
        db.upsert_download(&c).unwrap();

        db.delete_downloads_batch(&[b.request.id, c.request.id])
            .unwrap();
        let remaining = db.list_downloads().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].request.id, a.request.id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_platform_parse_os_release_major_distros() {
        let samples = [
            (
                "ID=ubuntu\nPRETTY_NAME=\"Ubuntu 26.04 LTS\"\nID_LIKE=debian\n",
                "ubuntu",
                "Ubuntu 26.04 LTS",
                vec!["debian"],
            ),
            (
                "ID=fedora\nPRETTY_NAME=\"Fedora Linux 44 (Workstation Edition)\"\n",
                "fedora",
                "Fedora Linux 44 (Workstation Edition)",
                vec![],
            ),
            (
                "ID=nobara\nPRETTY_NAME=\"Nobara Linux 44 (KDE Plasma Desktop Edition)\"\nID_LIKE=fedora\n",
                "nobara",
                "Nobara Linux 44 (KDE Plasma Desktop Edition)",
                vec!["fedora"],
            ),
            (
                "ID=arch\nPRETTY_NAME=\"Arch Linux\"\n",
                "arch",
                "Arch Linux",
                vec![],
            ),
            (
                "ID=alpine\nPRETTY_NAME=\"Alpine Linux v3.21\"\n",
                "alpine",
                "Alpine Linux v3.21",
                vec![],
            ),
            (
                "ID=opensuse-tumbleweed\nPRETTY_NAME=\"openSUSE Tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                "opensuse-tumbleweed",
                "openSUSE Tumbleweed",
                vec!["opensuse", "suse"],
            ),
            (
                "ID=debian\nPRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\n",
                "debian",
                "Debian GNU/Linux 13 (trixie)",
                vec![],
            ),
            (
                "ID=bazzite\nPRETTY_NAME=\"Bazzite (Fedora Atomic)\"\nID_LIKE=\"fedora\"\n",
                "bazzite",
                "Bazzite (Fedora Atomic)",
                vec!["fedora"],
            ),
            (
                "ID=tails\nPRETTY_NAME=\"Tails 6\"\nID_LIKE=debian\n",
                "tails",
                "Tails 6",
                vec!["debian"],
            ),
        ];

        for (raw, expected_id, expected_name, expected_like) in samples {
            let (id, name, like) = crate::platform::parse_os_release(raw);
            assert_eq!(id, expected_id);
            assert_eq!(name, expected_name);
            let expected_like_vec: Vec<String> =
                expected_like.into_iter().map(String::from).collect();
            assert_eq!(like, expected_like_vec);
        }
    }

    #[test]
    fn test_platform_ytdlp_assets_and_urls() {
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names("x86_64", "glibc", false),
            vec!["yt-dlp_linux", "yt-dlp"]
        );
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names("aarch64", "glibc", false),
            vec!["yt-dlp_linux_aarch64", "yt-dlp"]
        );
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names("armv7l", "glibc", false),
            vec!["yt-dlp_linux_armv7l", "yt-dlp"]
        );
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names("x86_64", "musl", false),
            vec!["yt-dlp"]
        );
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names("x86_64", "glibc", true),
            vec!["yt-dlp"]
        );
        assert_eq!(
            crate::updater::ytdlp_asset_url("yt-dlp_linux_aarch64"),
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64"
        );
        assert_eq!(
            crate::platform::preferred_ytdlp_asset_names_for_os("windows", "x86_64", "msvc", false),
            vec!["yt-dlp.exe"]
        );
        assert_eq!(
            crate::updater::ytdlp_asset_url("yt-dlp.exe"),
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        );
    }

    #[tokio::test]
    async fn test_cli_help_version_and_invalid_subcommand() {
        let none_args = vec!["vidgrab".to_string()];
        assert_eq!(crate::cli::try_run_cli_from_args(&none_args).await, None);

        let help_args = vec!["vidgrab".to_string(), "--help".to_string()];
        assert_eq!(crate::cli::try_run_cli_from_args(&help_args).await, Some(0));

        let ver_args = vec!["vidgrab".to_string(), "--version".to_string()];
        assert_eq!(crate::cli::try_run_cli_from_args(&ver_args).await, Some(0));

        let bad_args = vec!["vidgrab".to_string(), "invalid-subcommand".to_string()];
        assert_eq!(crate::cli::try_run_cli_from_args(&bad_args).await, Some(2));
    }

    #[test]
    fn test_flat_playlist_entry_thumbnail_resolution() {
        let json = r#"{
            "_type": "playlist",
            "id": "ytsearch12:test",
            "title": "ytsearch12:test",
            "entries": [
                {
                    "id": "MVF3Uci8qng",
                    "title": "Test Video 1",
                    "url": "https://www.youtube.com/watch?v=MVF3Uci8qng",
                    "thumbnail": null,
                    "thumbnails": [
                        {"url": "https://i.ytimg.com/vi/MVF3Uci8qng/hqdefault.jpg", "width": 360, "height": 202},
                        {"url": "https://i.ytimg.com/vi/MVF3Uci8qng/hq720.jpg", "width": 720, "height": 404}
                    ]
                },
                {
                    "id": "TDsFsf_J1WI",
                    "title": "Test Video 2 (fallback)",
                    "url": "https://www.youtube.com/watch?v=TDsFsf_J1WI",
                    "thumbnail": null,
                    "thumbnails": []
                }
            ]
        }"#;
        let parsed = crate::engine::parser::parse_video_info(json).expect("parse json");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(
            parsed.entries[0].thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/MVF3Uci8qng/hq720.jpg")
        );
        assert_eq!(
            parsed.entries[1].thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/TDsFsf_J1WI/hqdefault.jpg")
        );
        assert_eq!(
            parsed.thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/MVF3Uci8qng/hq720.jpg")
        );
    }
}
