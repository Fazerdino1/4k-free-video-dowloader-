//! FFmpeg media tools, subtitle cleaning, and Demucs AI audio commands.

use crate::commands::CmdResult;
use crate::logging::push_app_log;
use crate::AppState;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};
use vidgrab_core::ai_audio::{self, is_demucs_installed};
use vidgrab_core::media_tools;
use vidgrab_core::whisper::{self, WhisperFormat};
use vidgrab_shared::{AiAudioOperation, MediaToolAction};

#[tauri::command]
pub async fn cmd_run_media_tool(
    app: AppHandle,
    state: State<'_, AppState>,
    input_path: PathBuf,
    action: MediaToolAction,
) -> CmdResult<PathBuf> {
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Outil ffmpeg sur : {}", input_path.display()),
    );
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let out_path = media_tools::run_ffmpeg_tool(&state.paths, &settings, &input_path, &action)
        .await
        .map_err(|e| {
            push_app_log(&state.logs, Some(&app), format!("Erreur ffmpeg : {e}"));
            e.to_string()
        })?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Outil ffmpeg terminé : {}", out_path.display()),
    );
    Ok(out_path)
}

#[tauri::command]
pub async fn cmd_check_demucs(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(is_demucs_installed(&state.paths))
}

#[tauri::command]
pub async fn cmd_install_demucs(state: State<'_, AppState>) -> CmdResult<String> {
    if cfg!(target_os = "android") {
        return Err(
            "Demucs PyTorch n'est pas supporté sur Android ; l'application utilise automatiquement le moteur DSP ffmpeg-android intégré."
                .to_string(),
        );
    }
    ai_audio::install_demucs_venv(&state.paths)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_run_ai_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    input_path: PathBuf,
    operation: AiAudioOperation,
) -> CmdResult<PathBuf> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let app_for_prog = app.clone();
    ai_audio::process_audio_ai_with_progress(&state.paths, &settings, &input_path, operation, move |prog| {
        let _ = app_for_prog.emit("ai-audio-progress", &prog);
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_clean_subtitles_to_txt(file_path: PathBuf) -> CmdResult<PathBuf> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let sub_source = if matches!(ext.as_str(), "srt" | "vtt" | "ass") && file_path.exists() {
        file_path
    } else {
        let stem = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
        let mut found = None;
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let e2 = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .unwrap_or_default();
                if !stem.is_empty()
                    && name.starts_with(&stem)
                    && matches!(e2.as_str(), "srt" | "vtt" | "ass")
                {
                    found = Some(p);
                    break;
                }
            }
        }
        found.ok_or_else(|| {
            "Aucun fichier sous-titre (.srt / .vtt / .ass) trouvé pour cet élément.".to_string()
        })?
    };

    let raw = fs::read_to_string(&sub_source)
        .map_err(|e| format!("Lecture sous-titres impossible : {e}"))?;
    let cleaned = vidgrab_core::engine::parser::clean_subtitles_to_txt(&raw);
    if cleaned.is_empty() {
        return Err("Le fichier sous-titres ne contenait aucun texte exploitable.".to_string());
    }
    let out_txt = sub_source.with_extension("txt");
    fs::write(&out_txt, cleaned).map_err(|e| format!("Écriture .txt impossible : {e}"))?;
    Ok(out_txt)
}

#[tauri::command]
pub async fn cmd_run_whisper_transcribe(
    app: AppHandle,
    state: State<'_, AppState>,
    input_path: PathBuf,
    language: Option<String>,
    format: Option<String>,
) -> CmdResult<PathBuf> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let fmt = match format.as_deref().unwrap_or("srt") {
        "vtt" => WhisperFormat::Vtt,
        "txt" => WhisperFormat::Txt,
        _ => WhisperFormat::Srt,
    };
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Transcription Whisper locale : {}", input_path.display()),
    );
    let out = whisper::transcribe_media_local(
        &state.paths,
        &settings,
        &input_path,
        language.as_deref(),
        fmt,
    )
    .await
    .map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Transcription terminée : {}", out.display()),
    );
    Ok(out)
}
