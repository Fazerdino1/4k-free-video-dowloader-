//! Smart Mode (1-click instant download) resolution and quality fallback.
//!
//! When Smart Mode is enabled, any pasted URL bypasses the download configuration dialog
//! and uses the saved `SmartModeSettings`. If the requested resolution does not exist on
//! the video, Smart Mode automatically picks the highest available resolution less than or
//! equal to the target height (or the lowest available if all are higher).

use uuid::Uuid;
use vidgrab_shared::{DownloadRequest, DubbedTrackMode, LiveRecordMode, Settings, VideoInfo};

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
    let avail_heights = info.available_heights();
    let resolved_height = resolve_smart_height(sm.target_height, &avail_heights);

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

    let url = info
        .webpage_url
        .clone()
        .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={}", info.id));

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
        media_type: sm.media_type,
        target_height: resolved_height,
        explicit_format_id: None,
        priority: 0,
        prefer_60fps: true,
        prefer_hdr: false,
        preserve_vr_projection: false,
        video_container: sm.video_container,
        video_codec: sm.video_codec,
        audio_format: sm.audio_format,
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
