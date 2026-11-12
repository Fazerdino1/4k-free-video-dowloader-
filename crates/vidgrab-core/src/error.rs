//! Strongly-typed error definitions for `vidgrab-core`.
//!
//! No `unwrap()` or `expect()` is permitted in production code; all errors
//! propagate via `Result<T, CoreError>` and can be translated into clear,
//! localized user-facing messages.

use thiserror::Error;

/// Core domain error enum.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error(
        "DRM-protected content detected ({site}): Widevine/PlayReady streams cannot be downloaded."
    )]
    DrmProtected { site: String },

    #[error("Video is private or requires sign-in. Use the built-in Browser to log in.")]
    PrivateVideo,

    #[error("Video is restricted to channel members only.")]
    MembersOnly,

    #[error(
        "Age-restricted content. Sign in with the built-in Browser or enable browser cookies."
    )]
    AgeRestricted,

    #[error("Content is geo-blocked in your region. Try configuring a proxy or X-Forwarded-For country in Network settings.")]
    GeoBlocked,

    #[error("Video or URL not found (HTTP 404 or removed by uploader).")]
    VideoNotFound,

    #[error("Invalid or unsupported URL: {0}")]
    InvalidUrl(String),

    #[error("ffmpeg is missing from your system PATH. Install it on Nobara/Fedora with: sudo dnf install ffmpeg")]
    FfmpegMissing,

    #[error("yt-dlp binary not found at {0}. Click 'Update yt-dlp now' in Advanced settings.")]
    YtDlpMissing(String),

    #[error("SHA-256 checksum verification failed: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("yt-dlp execution failed: {0}")]
    YtDlpFailed(String),

    #[error("Audio AI processing failed: {0}")]
    AiProcessingFailed(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Network / HTTP error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML serialization error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML formatting error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl CoreError {
    /// Classifies raw `yt-dlp` stderr output into a user-friendly `CoreError`.
    pub fn from_yt_dlp_stderr(stderr: &str) -> Self {
        let lower = stderr.to_lowercase();
        if lower.contains("drm") || lower.contains("widevine") || lower.contains("playready") {
            return Self::DrmProtected {
                site: String::from("stream"),
            };
        }
        if lower.contains("private video")
            || lower.contains("video is private")
            || lower.contains("sign in if you've been granted access")
            || lower.contains("login required")
        {
            return Self::PrivateVideo;
        }
        if lower.contains("members-only")
            || lower.contains("join this channel to get access")
            || lower.contains("available to members")
        {
            return Self::MembersOnly;
        }
        if lower.contains("sign in to confirm your age")
            || lower.contains("age-restricted")
            || lower.contains("inappropriate for some users")
        {
            return Self::AgeRestricted;
        }
        if lower.contains("not available in your country")
            || lower.contains("geo-restricted")
            || lower.contains("blocked in your country")
        {
            return Self::GeoBlocked;
        }
        if lower.contains("video unavailable")
            || lower.contains("http error 404")
            || lower.contains("has been removed")
        {
            return Self::VideoNotFound;
        }
        let cleaned = stderr
            .lines()
            .rev()
            .find(|l| l.contains("ERROR:"))
            .unwrap_or(stderr)
            .trim()
            .to_string();
        Self::YtDlpFailed(cleaned)
    }

    /// Checks whether a URL belongs to a known Widevine DRM streaming service.
    pub fn check_drm_url(raw_url: &str) -> CoreResult<()> {
        let parsed = url::Url::parse(raw_url).map_err(|_| Self::InvalidUrl(raw_url.to_string()))?;
        let host = parsed.host_str().unwrap_or_default().to_lowercase();
        let drm_hosts = [
            ("netflix.com", "Netflix"),
            ("disneyplus.com", "Disney+"),
            ("primevideo.com", "Amazon Prime Video"),
            ("max.com", "Max / HBO"),
            ("hbomax.com", "HBO Max"),
            ("hulu.com", "Hulu"),
            ("tv.apple.com", "Apple TV+"),
            ("canalplus.com", "Canal+"),
            ("open.spotify.com", "Spotify (Widevine)"),
        ];
        for (domain, name) in drm_hosts {
            if host == domain || host.ends_with(&format!(".{domain}")) {
                return Err(Self::DrmProtected {
                    site: name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Returns `true` when the failure is likely temporary (network hiccup, timeout, HTTP 429/5xx)
    /// and safe to retry with exponential backoff, rather than a permanent content/auth restriction.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        if matches!(self, Self::DrmProtected { .. }) {
            return false;
        }
        let msg = self.to_string().to_ascii_lowercase();
        let fatal_markers = [
            "drm",
            "private",
            "privée",
            "members-only",
            "members only",
            "réservé aux membres",
            "age-restricted",
            "confirm your age",
            "geo",
            "country",
            "pays",
            "404",
            "not found",
            "introuvable",
            "unavailable",
            "indisponible",
            "copyright",
            "invalid url",
            "url invalide",
            "cancelled",
            "annulé",
            "paused",
            "en pause",
        ];
        if fatal_markers.iter().any(|m| msg.contains(m)) {
            return false;
        }
        let transient_markers = [
            "429",
            "too many requests",
            "500",
            "502",
            "503",
            "504",
            "http error 5",
            "timed out",
            "timeout",
            "délai",
            "connection reset",
            "connection refused",
            "connection aborted",
            "broken pipe",
            "unexpected eof",
            "early eof",
            "network",
            "réseau",
            "temporary failure",
            "name resolution",
            "urlopen error",
            "unable to download",
            "remote end closed",
            "try again",
            "socket",
            "transport error",
        ];
        transient_markers.iter().any(|m| msg.contains(m))
    }
}
