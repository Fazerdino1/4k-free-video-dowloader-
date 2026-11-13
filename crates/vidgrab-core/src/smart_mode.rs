//! Smart Mode (1-click instant download) resolution and quality fallback.
//!
//! When Smart Mode is enabled, any pasted URL bypasses the download configuration dialog
//! and uses the saved `SmartModeSettings`. If the requested resolution does not exist on
//! the video, Smart Mode automatically picks the highest available resolution less than or
//! equal to the target height (or the lowest available if all are higher).

use uuid::Uuid;
use vidgrab_shared::{
    DomainProfile, DownloadRequest, DubbedTrackMode, LiveRecordMode, Settings, VideoInfo,
};

/// Finds the first [`DomainProfile`] whose `domain` matches the URL host.
pub fn match_domain_profile<'a>(
    url_str: &str,
    profiles: &'a [DomainProfile],
) -> Option<&'a DomainProfile> {
    let parsed = url::Url::parse(url_str).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    profiles.iter().find(|p| {
        let d = p.domain.trim().to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}

/// Resolves the best available video height matching `requested_height` from `available_heights`.
///
/// If the exact `requested_height` is not available, returns the highest resolution strictly
/// below `requested_height` ("meilleure qualité inférieure"). If all available resolutions
/// are higher than `requested_height`, returns the lowest available resolution.
pub fn resolve_smart_height(
    requested_height: Option<u32>,
    available_heights: &[u32],
) -> Option<u32> {
    let target = requested_height?;
    if available_heights.is_empty() {
        return Some(target);
    }

    let mut sorted = available_heights.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a)); // descending
    sorted.dedup();

    if let Some(&chosen) = sorted.iter().find(|&&h| h <= target) {
        Some(chosen)
    } else {
        sorted.last().copied()
    }
}

/// Constructs a ready-to-queue [`DownloadRequest`] from [`VideoInfo`] and [`Settings`]
/// using the user's Smart Mode preferences.
pub fn build_smart_download_request(info: &VideoInfo, settings: &Settings) -> DownloadRequest {
    let sm = &settings.smart_mode;
    let url = info
        .webpage_url
        .clone()
        .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={}", info.id));
    let matched_profile = match_domain_profile(&url, &settings.domain_profiles);

    let effective_media_type = matched_profile.map(|p| p.media_type).unwrap_or(sm.media_type);
    let requested_height = matched_profile
        .map(|p| p.target_height)
        .unwrap_or(sm.target_height);
    let effective_container = matched_profile
        .map(|p| p.video_container)
        .unwrap_or(sm.video_container);
    let effective_audio_fmt = matched_profile
        .map(|p| p.audio_format)
        .unwrap_or(sm.audio_format);

    let avail_heights = info.available_heights();
    let resolved_height = resolve_smart_height(requested_height, &avail_heights);

    let mut audio_languages = Vec::new();
    if let Some(pref_audio) = &sm.preferred_audio_language {
        if !pref_audio.trim().is_empty() {
            let available_langs = info.available_audio_languages();
            if available_langs.is_empty()
                || available_langs
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case(pref_audio))
            {
                audio_languages.push(pref_audio.trim().to_string());
            }
        }
    }

    let mut subtitle_languages = Vec::new();
    let mut include_auto_subtitles = false;
    if let Some(pref_sub) = &sm.preferred_subtitle_language {
        let lang = pref_sub.trim();
        if !lang.is_empty() {
            subtitle_languages.push(lang.to_string());
            if !info.subtitles.contains_key(lang) && info.automatic_captions.contains_key(lang) {
                include_auto_subtitles = true;
            }
        }
    }

    DownloadRequest {
        id: Uuid::new_v4(),
        url,
        title: if info.title.is_empty() {
            String::from("Smart Download")
        } else {
            info.title.clone()
        },
        thumbnail: info.thumbnail.clone(),
        group_id: None,
        group_title: None,
        playlist_index: None,
        media_type: effective_media_type,
        target_height: resolved_height,
        explicit_format_id: None,
        priority: 0,
        prefer_60fps: true,
        prefer_hdr: false,
        preserve_vr_projection: false,
        video_container: effective_container,
        video_codec: sm.video_codec,
        audio_format: effective_audio_fmt,
        audio_bitrate: sm.audio_bitrate,
        audio_languages,
        dubbed_track_mode: DubbedTrackMode::SingleTrack,
        subtitle_languages,
        include_auto_subtitles,
        subtitle_format: sm.subtitle_format,
        embed_subtitles: sm.embed_subtitles,
        output_dir: sm.output_dir.clone(),
        filename_template: settings.filename_template.clone(),
        subfolder_name: None,
        number_playlist_files: false,
        generate_m3u: false,
        live_record_mode: LiveRecordMode::FromNow,
        sponsorblock_remove: settings.sponsorblock_enabled,
        clip_start: None,
        clip_end: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidgrab_shared::{AudioFormat, MediaType, VideoContainer};

    #[test]
    fn test_domain_profile_matching() {
        let mut settings = Settings::default();
        settings.domain_profiles.push(DomainProfile {
            domain: "soundcloud.com".to_string(),
            media_type: MediaType::AudioOnly,
            target_height: None,
            video_container: VideoContainer::Mp4,
            audio_format: AudioFormat::Flac,
        });
        let info = VideoInfo {
            id: "123".to_string(),
            title: "Track".to_string(),
            webpage_url: Some("https://soundcloud.com/artist/track".to_string()),
            ..Default::default()
        };
        let req = build_smart_download_request(&info, &settings);
        assert_eq!(req.media_type, MediaType::AudioOnly);
        assert_eq!(req.audio_format, AudioFormat::Flac);
    }
}
