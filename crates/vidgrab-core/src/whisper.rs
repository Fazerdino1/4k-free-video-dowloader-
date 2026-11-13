//! Local Whisper speech-to-text transcription module (`whisper-cli` / `faster-whisper`).
//!
//! Generates `.srt`, `.vtt`, or `.txt` transcripts offline without cloud APIs.

use crate::config::AppPaths;
use crate::error::{CoreError, CoreResult};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Supported Whisper output transcript formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperFormat {
    Srt,
    Vtt,
    Txt,
}

impl WhisperFormat {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Txt => "txt",
        }
    }
}

/// Locates an available Whisper CLI binary (`whisper-cli`, `whisper`, or `faster-whisper-cli`).
#[must_use]
pub fn find_whisper_binary() -> Option<PathBuf> {
    for candidate in ["whisper-cli", "whisper", "faster-whisper-cli", "whisper-ctranslate2"] {
        if let Ok(path) = which::which(candidate) {
            return Some(path);
        }
    }
    None
}

/// Builds CLI arguments for `whisper` / `whisper-cli`.
#[must_use]
pub fn build_whisper_args(
    input_wav: &Path,
    output_dir: &Path,
    language: Option<&str>,
    format: WhisperFormat,
) -> Vec<String> {
    let mut args = vec![
        input_wav.to_string_lossy().into_owned(),
        "--output_dir".to_string(),
        output_dir.to_string_lossy().into_owned(),
        "--output_format".to_string(),
        format.as_str().to_string(),
    ];
    if let Some(lang) = language.map(str::trim).filter(|l| !l.is_empty() && *l != "auto") {
        args.push("--language".to_string());
        args.push(lang.to_string());
    }
    args
}

/// Transcribes `input_media` locally using Whisper (converting to 16 kHz mono WAV via `ffmpeg` first).
pub async fn transcribe_media_local(
    paths: &AppPaths,
    settings: &vidgrab_shared::Settings,
    input_media: &Path,
    language: Option<&str>,
    format: WhisperFormat,
) -> CoreResult<PathBuf> {
    if !input_media.exists() {
        return Err(CoreError::AiProcessingFailed(format!(
            "Fichier introuvable : {}",
            input_media.display()
        )));
    }
    let whisper_bin = find_whisper_binary().ok_or_else(|| {
        CoreError::AiProcessingFailed(
            "Aucun binaire Whisper local détecté (`whisper-cli` ou `whisper`). Installez `openai-whisper` ou `whisper-ctranslate2`."
                .to_string(),
        )
    })?;

    let ffmpeg = paths.effective_ffmpeg(settings)?;
    let parent = input_media.parent().unwrap_or_else(|| Path::new("."));
    let stem = input_media
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript");
    let temp_wav = parent.join(format!("{stem}_whisper_16k.wav"));

    let mut ff_cmd = Command::new(&ffmpeg);
    ff_cmd
        .args([
            "-y",
            "-i",
            &input_media.to_string_lossy(),
            "-vn",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            &temp_wav.to_string_lossy(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut ff_cmd);
    let ff_out = ff_cmd.output().await?;
    if !ff_out.status.success() {
        return Err(CoreError::AiProcessingFailed(format!(
            "Conversion WAV 16kHz échouée : {}",
            String::from_utf8_lossy(&ff_out.stderr)
        )));
    }

    let args = build_whisper_args(&temp_wav, parent, language, format);
    let mut w_cmd = Command::new(&whisper_bin);
    w_cmd
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::configure_background_command(&mut w_cmd);
    let w_out = w_cmd.output().await?;
    let _ = tokio::fs::remove_file(&temp_wav).await;

    if !w_out.status.success() {
        return Err(CoreError::AiProcessingFailed(format!(
            "Whisper a échoué : {}",
            String::from_utf8_lossy(&w_out.stderr)
        )));
    }

    let expected = parent.join(format!("{stem}_whisper_16k.{}", format.as_str()));
    let final_out = parent.join(format!("{stem}.{}", format.as_str()));
    if expected.exists() {
        let _ = tokio::fs::rename(&expected, &final_out).await;
    }
    Ok(final_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_whisper_args() {
        let args = build_whisper_args(
            Path::new("/tmp/audio.wav"),
            Path::new("/tmp"),
            Some("fr"),
            WhisperFormat::Srt,
        );
        assert!(args.contains(&"--output_format".to_string()));
        assert!(args.contains(&"srt".to_string()));
        assert!(args.contains(&"--language".to_string()));
        assert!(args.contains(&"fr".to_string()));
    }
}
