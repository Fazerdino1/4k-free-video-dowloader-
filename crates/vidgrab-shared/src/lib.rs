//! Shared types between `vidgrab-core`, `src-tauri`, and `vidgrab-ui`.
//!
//! All data structures serialized over Tauri IPC commands and events live here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Type of media download requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    #[default]
    Video,
    AudioOnly,
    SubtitlesOnly,
}

/// Video container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoContainer {
    #[default]
    Mp4,
    Mkv,
}

impl VideoContainer {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
        }
    }
}

/// Preferred video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    Auto,
    H264,
    Vp9,
    Av1,
}

impl VideoCodec {
    pub fn yt_dlp_filter(&self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::H264 => Some("avc1"),
            Self::Vp9 => Some("vp9"),
            Self::Av1 => Some("av01"),
        }
    }
}

/// Audio format for audio extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Mp3,
    M4a,
    Ogg,
    Opus,
    Flac,
    Wav,
}

impl AudioFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Ogg => "vorbis",
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Ogg => "ogg",
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }
}

/// Audio bitrate in kbps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioBitrate {
    Kbps128,
    Kbps192,
    Kbps256,
    #[default]
    Kbps320,
}

impl AudioBitrate {
    pub fn as_yt_dlp_quality(&self) -> &'static str {
        match self {
            Self::Kbps128 => "128K",
            Self::Kbps192 => "192K",
            Self::Kbps256 => "256K",
            Self::Kbps320 => "320K",
        }
    }

    pub fn kbps(&self) -> u16 {
        match self {
            Self::Kbps128 => 128,
            Self::Kbps192 => 192,
            Self::Kbps256 => 256,
            Self::Kbps320 => 320,
        }
    }
}

/// Subtitle output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleFormat {
    #[default]
    Srt,
    Vtt,
    Ass,
}

impl SubtitleFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Ass => "ass",
        }
    }
}

/// How multiple dubbed audio tracks should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DubbedTrackMode {
    #[default]
    SingleTrack,
    SeparateFiles,
    MultiTrackMkv,
}

/// Live stream recording mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LiveRecordMode {
    #[default]
    FromNow,
    FromStart,
}

/// 3D / VR projection metadata detected on a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionType {
    #[default]
    Rectangular,
    Equirectangular360,
    Cubemap360,
    Mesh360,
    Stereo3D,
}

/// Information about a single media format returned by `yt-dlp`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Format {
    #[serde(deserialize_with = "null_to_default")]
    pub format_id: String,
    #[serde(deserialize_with = "null_to_default")]
    pub ext: String,
    pub resolution: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub abr: Option<f32>,
    pub vbr: Option<f32>,
    pub tbr: Option<f32>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    pub format_note: Option<String>,
    pub dynamic_range: Option<String>,
    pub language: Option<String>,
    pub projection: Option<String>,
}

impl Format {
    /// Returns the best available size estimate in bytes.
    pub fn estimated_size(&self) -> Option<u64> {
        self.filesize.or(self.filesize_approx)
    }

    /// True if this format contains a video stream.
    pub fn has_video(&self) -> bool {
        self.vcodec.as_deref().map(|c| c != "none").unwrap_or(false)
    }

    /// True if this format contains an audio stream.
    pub fn has_audio(&self) -> bool {
        self.acodec.as_deref().map(|c| c != "none").unwrap_or(false)
    }

    /// True if HDR (HDR10, HLG, Dolby Vision).
    pub fn is_hdr(&self) -> bool {
        self.dynamic_range
            .as_deref()
            .map(|d| !d.eq_ignore_ascii_case("sdr") && !d.is_empty())
            .unwrap_or(false)
    }

    /// Quality label for UI display (e.g., "4K (2160p) 60fps HDR").
    pub fn display_quality(&self) -> String {
        let height = self.height.unwrap_or(0);
        let base = match height {
            4320.. => format!("8K ({height}p)"),
            2160..=4319 => format!("4K ({height}p)"),
            1440..=2159 => format!("2K ({height}p)"),
            1080..=1439 => "Full HD (1080p)".to_string(),
            720..=1079 => "HD (720p)".to_string(),
            height if height > 0 => format!("{height}p"),
            _ => self
                .format_note
                .clone()
                .unwrap_or_else(|| self.format_id.clone()),
        };
        let mut parts = vec![base];
        if let Some(fps) = self.fps {
            if fps > 30.0 {
                parts.push(format!("{}fps", fps.round() as u32));
            }
        }
        if self.is_hdr() {
            if let Some(dr) = &self.dynamic_range {
                parts.push(dr.clone());
            }
        }
        parts.join(" ")
    }
}

/// Subtitle track entry from `yt-dlp`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SubtitleTrack {
    #[serde(deserialize_with = "null_to_default")]
    pub ext: String,
    pub url: Option<String>,
    pub name: Option<String>,
}

/// Chapter marker inside a video.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Chapter {
    #[serde(deserialize_with = "null_to_default")]
    pub title: String,
    #[serde(deserialize_with = "null_to_default")]
    pub start_time: f64,
    #[serde(deserialize_with = "null_to_default")]
    pub end_time: f64,
}

/// Thumbnail candidate entry returned by `yt-dlp` in `thumbnails` arrays.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ThumbnailCandidate {
    #[serde(deserialize_with = "null_to_default")]
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub preference: Option<i32>,
}

/// Entry inside a playlist or channel listing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlaylistEntry {
    #[serde(deserialize_with = "null_to_default")]
    pub id: String,
    #[serde(deserialize_with = "null_to_default")]
    pub title: String,
    pub url: Option<String>,
    pub webpage_url: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_ignore_nulls")]
    pub thumbnails: Vec<ThumbnailCandidate>,
    pub view_count: Option<u64>,
}

impl PlaylistEntry {
    /// Effective URL to download this entry.
    pub fn effective_url(&self) -> String {
        if let Some(url) = &self.webpage_url {
            if !url.is_empty() {
                return url.clone();
            }
        }
        if let Some(url) = &self.url {
            if !url.is_empty() {
                return url.clone();
            }
        }
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    let opt = Option::<T>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

fn deserialize_subtitles_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Vec<SubtitleTrack>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<HashMap<String, Option<Vec<Option<SubtitleTrack>>>>> =
        Option::deserialize(deserializer)?;
    let Some(raw_map) = opt else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::with_capacity(raw_map.len());
    for (k, v_opt) in raw_map {
        let tracks: Vec<SubtitleTrack> = v_opt.unwrap_or_default().into_iter().flatten().collect();
        out.insert(k, tracks);
    }
    Ok(out)
}

fn deserialize_vec_ignore_nulls<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let opt: Option<Vec<Option<T>>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default().into_iter().flatten().collect())
}

/// Parsed output of `yt-dlp --dump-single-json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VideoInfo {
    #[serde(deserialize_with = "null_to_default")]
    pub id: String,
    #[serde(deserialize_with = "null_to_default")]
    pub title: String,
    pub uploader: Option<String>,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub channel_url: Option<String>,
    pub upload_date: Option<String>,
    pub duration: Option<f64>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_ignore_nulls")]
    pub thumbnails: Vec<ThumbnailCandidate>,
    pub webpage_url: Option<String>,
    pub extractor: Option<String>,
    pub extractor_key: Option<String>,
    pub is_live: Option<bool>,
    pub was_live: Option<bool>,
    #[serde(deserialize_with = "deserialize_vec_ignore_nulls")]
    pub formats: Vec<Format>,
    #[serde(deserialize_with = "deserialize_subtitles_map")]
    pub subtitles: HashMap<String, Vec<SubtitleTrack>>,
    #[serde(deserialize_with = "deserialize_subtitles_map")]
    pub automatic_captions: HashMap<String, Vec<SubtitleTrack>>,
    #[serde(deserialize_with = "deserialize_vec_ignore_nulls")]
    pub chapters: Vec<Chapter>,
    #[serde(rename = "_type")]
    pub result_type: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_ignore_nulls")]
    pub entries: Vec<PlaylistEntry>,
}

impl VideoInfo {
    /// True if this metadata represents a playlist, channel, or multi-video feed.
    pub fn is_playlist(&self) -> bool {
        self.result_type.as_deref() == Some("playlist")
            || self.result_type.as_deref() == Some("multi_video")
            || !self.entries.is_empty()
    }

    /// Available distinct video heights sorted descending (e.g. [4320, 2160, 1440, 1080, 720, 480, 360, 240, 144]).
    pub fn available_heights(&self) -> Vec<u32> {
        let mut heights: Vec<u32> = self
            .formats
            .iter()
            .filter(|f| f.has_video())
            .filter_map(|f| f.height)
            .collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));
        heights.dedup();
        heights
    }

    /// Available audio languages for multi-language dubbed tracks.
    pub fn available_audio_languages(&self) -> Vec<String> {
        let mut langs: Vec<String> = self
            .formats
            .iter()
            .filter(|f| f.has_audio())
            .filter_map(|f| f.language.clone())
            .filter(|l| !l.is_empty() && l != "und")
            .collect();
        langs.sort();
        langs.dedup();
        langs
    }

    /// Detects if any stream advertises 360/VR or 3D projection.
    pub fn detected_projection(&self) -> ProjectionType {
        for f in &self.formats {
            if let Some(proj) = &f.projection {
                let p = proj.to_lowercase();
                if p.contains("equirectangular") || p.contains("360") {
                    return ProjectionType::Equirectangular360;
                } else if p.contains("cubemap") {
                    return ProjectionType::Cubemap360;
                } else if p.contains("mesh") {
                    return ProjectionType::Mesh360;
                } else if p.contains("3d") || p.contains("stereo") {
                    return ProjectionType::Stereo3D;
                }
            }
        }
        ProjectionType::Rectangular
    }
}

/// Complete specification of a download request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadRequest {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub group_id: Option<Uuid>,
    pub group_title: Option<String>,
    pub playlist_index: Option<u32>,
    pub media_type: MediaType,
    /// Desired height in pixels (e.g., 2160 for 4K, 1080 for Full HD). `None` means best available.
    pub target_height: Option<u32>,
    /// Explicit yt-dlp `-f` format expression selected in Expert Mode.
    pub explicit_format_id: Option<String>,
    /// Higher numbers are placed higher in the queue.
    pub priority: i32,
    pub prefer_60fps: bool,
    pub prefer_hdr: bool,
    pub preserve_vr_projection: bool,
    pub video_container: VideoContainer,
    pub video_codec: VideoCodec,
    pub audio_format: AudioFormat,
    pub audio_bitrate: AudioBitrate,
    pub audio_languages: Vec<String>,
    pub dubbed_track_mode: DubbedTrackMode,
    pub subtitle_languages: Vec<String>,
    pub include_auto_subtitles: bool,
    pub subtitle_format: SubtitleFormat,
    pub embed_subtitles: bool,
    pub output_dir: PathBuf,
    pub filename_template: String,
    pub subfolder_name: Option<String>,
    pub number_playlist_files: bool,
    pub generate_m3u: bool,
    pub live_record_mode: LiveRecordMode,
    pub sponsorblock_remove: bool,
    /// Optional start timestamp (`HH:MM:SS`, `MM:SS`, or seconds) for partial section downloads.
    pub clip_start: Option<String>,
    /// Optional end timestamp (`HH:MM:SS`, `MM:SS`, or seconds) for partial section downloads.
    pub clip_end: Option<String>,
}

impl Default for DownloadRequest {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            url: String::new(),
            title: String::from("Untitled"),
            thumbnail: None,
            group_id: None,
            group_title: None,
            playlist_index: None,
            media_type: MediaType::Video,
            target_height: Some(1080),
            explicit_format_id: None,
            priority: 0,
            prefer_60fps: true,
            prefer_hdr: false,
            preserve_vr_projection: false,
            video_container: VideoContainer::Mp4,
            video_codec: VideoCodec::Auto,
            audio_format: AudioFormat::Mp3,
            audio_bitrate: AudioBitrate::Kbps320,
            audio_languages: Vec::new(),
            dubbed_track_mode: DubbedTrackMode::SingleTrack,
            subtitle_languages: Vec::new(),
            include_auto_subtitles: false,
            subtitle_format: SubtitleFormat::Srt,
            embed_subtitles: false,
            output_dir: PathBuf::from("."),
            filename_template: String::from("%(title)s [%(id)s].%(ext)s"),
            subfolder_name: None,
            number_playlist_files: false,
            generate_m3u: false,
            live_record_mode: LiveRecordMode::FromNow,
            sponsorblock_remove: false,
            clip_start: None,
            clip_end: None,
        }
    }
}

/// Current state of an item in the download queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    #[default]
    Queued,
    Downloading,
    Paused,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

/// Real-time progress emitted during a download.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DownloadProgress {
    pub id: Uuid,
    pub status: DownloadStatus,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<u64>,
    pub percent: f32,
    pub fragment_index: Option<u32>,
    pub fragment_count: Option<u32>,
    pub output_path: Option<PathBuf>,
    pub error_message: Option<String>,
    pub retry_count: u8,
    pub max_retries: u8,
}

/// Persisted download item for UI display, SQLite storage, and JSON export/import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub request: DownloadRequest,
    pub progress: DownloadProgress,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Duplicate file handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DuplicatePolicy {
    Skip,
    #[default]
    Rename,
    Overwrite,
}

/// Post-download completion action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PostDownloadAction {
    #[default]
    Nothing,
    Notify,
    ShutdownSystem,
}

/// UI Theme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Oled,
    Light,
}

/// IP protocol version forcing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IpVersion {
    #[default]
    Auto,
    Ipv4Only,
    Ipv6Only,
}

/// Browser choice for `--cookies-from-browser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CookieBrowser {
    #[default]
    None,
    InternalWebview,
    Firefox,
    Chrome,
    Brave,
    Chromium,
}

impl CookieBrowser {
    pub fn yt_dlp_name(&self) -> Option<&'static str> {
        match self {
            Self::None | Self::InternalWebview => None,
            Self::Firefox => Some("firefox"),
            Self::Chrome => Some("chrome"),
            Self::Brave => Some("brave"),
            Self::Chromium => Some("chromium"),
        }
    }
}

/// Proxy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProxyConfig {
    pub enabled: bool,
    /// Full proxy URL e.g. `http://user:pass@127.0.0.1:8080` or `socks5://127.0.0.1:1080`
    pub url: String,
    /// If non-empty, only use proxy when the target URL domain matches one of these domains.
    pub only_domains: Vec<String>,
}

/// Smart Mode preferences (1-click instant download without dialog).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartModeSettings {
    pub enabled: bool,
    pub media_type: MediaType,
    pub target_height: Option<u32>,
    pub video_container: VideoContainer,
    pub video_codec: VideoCodec,
    pub audio_format: AudioFormat,
    pub audio_bitrate: AudioBitrate,
    pub preferred_audio_language: Option<String>,
    pub preferred_subtitle_language: Option<String>,
    pub subtitle_format: SubtitleFormat,
    pub embed_subtitles: bool,
    pub output_dir: PathBuf,
}

impl Default for SmartModeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            media_type: MediaType::Video,
            target_height: Some(1080),
            video_container: VideoContainer::Mp4,
            video_codec: VideoCodec::Auto,
            audio_format: AudioFormat::Mp3,
            audio_bitrate: AudioBitrate::Kbps320,
            preferred_audio_language: None,
            preferred_subtitle_language: None,
            subtitle_format: SubtitleFormat::Srt,
            embed_subtitles: false,
            output_dir: PathBuf::from("."),
        }
    }
}

/// Global application settings stored in `~/.config/vidgrab/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // General
    pub language: String,
    pub theme: ThemePreference,
    pub accent_color: String,
    pub autostart: bool,
    pub minimize_to_tray: bool,
    pub auto_detect_clipboard: bool,
    pub notifications_enabled: bool,
    pub post_download_action: PostDownloadAction,

    // Downloads
    pub video_output_dir: PathBuf,
    pub audio_output_dir: PathBuf,
    pub filename_template: String,
    pub concurrent_downloads: usize,
    pub concurrent_fragments: u8,
    /// Speed limit in KiB/s (`0` = unlimited).
    pub speed_limit_kbps: u64,
    pub duplicate_policy: DuplicatePolicy,
    pub auto_organize_by_channel: bool,
    pub schedule_enabled: bool,
    pub schedule_start_hour: u8,
    pub schedule_end_hour: u8,
    /// Maximum automatic retry attempts with exponential backoff on transient errors.
    pub max_auto_retries: u8,

    // Files & Metadata
    pub embed_thumbnail: bool,
    pub embed_metadata: bool,
    pub embed_chapters: bool,
    pub split_chapters: bool,
    pub write_description: bool,

    // Network & Cookies
    pub proxy: ProxyConfig,
    pub source_address: Option<String>,
    pub ip_version: IpVersion,
    pub geo_bypass_country: Option<String>,
    pub user_agent: Option<String>,
    pub cookie_browser: CookieBrowser,
    pub custom_cookies_path: Option<PathBuf>,

    // Subscriptions
    pub default_subscription_interval_mins: u32,
    pub default_subscription_max_videos: u32,

    // Advanced
    pub use_system_yt_dlp: bool,
    pub custom_yt_dlp_path: Option<PathBuf>,
    pub custom_ffmpeg_path: Option<PathBuf>,
    pub auto_update_yt_dlp: bool,
    pub custom_yt_dlp_args: Vec<String>,
    pub log_level: String,
    pub sponsorblock_enabled: bool,

    // Smart Mode
    pub smart_mode: SmartModeSettings,

    // Profiles & Integrity
    #[serde(default)]
    pub verify_downloaded_media: bool,
    #[serde(default)]
    pub domain_profiles: Vec<DomainProfile>,
    #[serde(default)]
    pub post_process_pipeline: PostProcessPipeline,
}

/// Automated post-download media processing pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PostProcessPipeline {
    pub normalize_loudness: bool,
    pub remove_silence: bool,
    pub transcode_hw_h264: bool,
}

/// Per-domain quality/format override profile used by Smart Mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DomainProfile {
    pub domain: String,
    pub media_type: MediaType,
    pub target_height: Option<u32>,
    pub video_container: VideoContainer,
    pub audio_format: AudioFormat,
}

impl Default for DomainProfile {
    fn default() -> Self {
        Self {
            domain: String::new(),
            media_type: MediaType::Video,
            target_height: Some(1080),
            video_container: VideoContainer::Mp4,
            audio_format: AudioFormat::Mp3,
        }
    }
}

/// Real-time stage/progress event emitted during AI audio processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAudioProgress {
    pub stage: String,
    pub percent: f32,
    pub message: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: String::from("fr"),
            theme: ThemePreference::System,
            accent_color: String::from("#10b981"),
            autostart: false,
            minimize_to_tray: true,
            auto_detect_clipboard: true,
            notifications_enabled: true,
            post_download_action: PostDownloadAction::Notify,

            video_output_dir: PathBuf::from("Videos"),
            audio_output_dir: PathBuf::from("Music"),
            filename_template: String::from("%(title)s [%(id)s].%(ext)s"),
            concurrent_downloads: 3,
            concurrent_fragments: 4,
            speed_limit_kbps: 0,
            duplicate_policy: DuplicatePolicy::Rename,
            auto_organize_by_channel: false,
            schedule_enabled: false,
            schedule_start_hour: 1,
            schedule_end_hour: 7,
            max_auto_retries: 2,

            embed_thumbnail: true,
            embed_metadata: true,
            embed_chapters: true,
            split_chapters: false,
            write_description: false,

            proxy: ProxyConfig::default(),
            source_address: None,
            ip_version: IpVersion::Auto,
            geo_bypass_country: None,
            user_agent: None,
            cookie_browser: CookieBrowser::None,
            custom_cookies_path: None,

            default_subscription_interval_mins: 60,
            default_subscription_max_videos: 10,

            use_system_yt_dlp: false,
            custom_yt_dlp_path: None,
            custom_ffmpeg_path: None,
            auto_update_yt_dlp: true,
            custom_yt_dlp_args: Vec::new(),
            log_level: String::from("info"),
            sponsorblock_enabled: false,

            smart_mode: SmartModeSettings::default(),
            verify_downloaded_media: false,
            domain_profiles: Vec::new(),
            post_process_pipeline: PostProcessPipeline::default(),
        }
    }
}

/// Subscription to a channel or playlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub check_interval_mins: u32,
    pub media_type: MediaType,
    pub target_height: Option<u32>,
    pub video_container: VideoContainer,
    pub audio_format: AudioFormat,
    pub output_dir: PathBuf,
    pub max_videos: u32,
    pub include_shorts: bool,
    pub include_lives: bool,
    pub paused: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// AI Audio processing operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAudioOperation {
    /// Remove vocals (Karaoke / instrumental track).
    RemoveVocals,
    /// Extract vocals only (A cappella).
    ExtractVocals,
    /// Isolate vocals from background noise.
    IsolateVoiceNoise,
    /// Remove background music while keeping speech.
    RemoveBackgroundMusic,
    /// Reduce room echo and reverb via ffmpeg `afftdn` / `arnndn`.
    ReduceEchoReverb,
}

/// Netscape HTTP Cookie entry exported from the built-in browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub domain: String,
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    pub expires: i64,
    pub name: String,
    pub value: String,
}

/// Export bundle for backing up or migrating downloads and subscriptions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub downloads: Vec<DownloadItem>,
    pub subscriptions: Vec<Subscription>,
}

/// Built-in post-download FFmpeg media toolbox operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaToolAction {
    /// Losslessly remux stream containers into `.mp4` (`-c copy -movflags +faststart`).
    RemuxMp4,
    /// Extract audio track into 320 kbps MP3.
    ExtractAudioMp3,
    /// Extract lossless audio track into FLAC.
    ExtractAudioFlac,
    /// Re-encode into compact H.264/AAC MP4 (`-crf 24`).
    CompressH264,
    /// Trim an existing file between two timestamps using stream copy.
    TrimClip { start: String, end: String },
}

/// Runtime Linux host, architecture, libc, sandbox, and display compatibility summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxPlatformInfo {
    pub distro_id: String,
    pub distro_name: String,
    pub distro_like: Vec<String>,
    pub arch: String,
    pub libc: String,
    pub init_system: String,
    pub display_backend: String,
    pub sandbox: String,
    pub is_immutable: bool,
    pub has_nvidia: bool,
    pub is_qubes: bool,
    pub is_tails: bool,
    pub recommended_tor_proxy: Option<String>,
    pub yt_dlp_path: Option<String>,
    pub yt_dlp_asset: String,
    pub ffmpeg_path: Option<String>,
}
