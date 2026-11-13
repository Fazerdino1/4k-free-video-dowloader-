//! Download queue, URL analysis, bulk import, and JSON export/import commands.

use crate::commands::CmdResult;
use crate::logging::push_app_log;
use crate::AppState;
use chrono::Timelike;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use uuid::Uuid;
use vidgrab_core::engine;
use vidgrab_core::smart_mode::build_smart_download_request;
use vidgrab_shared::{DownloadItem, DownloadRequest, VideoInfo};

#[tauri::command]
pub async fn cmd_analyze_url(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    flat_playlist: bool,
) -> CmdResult<VideoInfo> {
    if !flat_playlist {
        if let Ok(Some(cached)) = state.db.get_cached_analysis(&url, 900) {
            push_app_log(&state.logs, Some(&app), format!("Analyse (cache) : {url}"));
            return Ok(cached);
        }
    }
    push_app_log(&state.logs, Some(&app), format!("Analyse : {url}"));
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let mut info = match engine::analyze_url(&state.paths, &settings, &url, flat_playlist).await {
        Ok(i) => i,
        Err(e) => {
            push_app_log(&state.logs, Some(&app), format!("Erreur analyse : {e}"));
            return Err(e.to_string());
        }
    };
    const MAX_PREFETCH_ENTRY_THUMBS: usize = 30;
    let mut entry_tasks = Vec::new();
    for (idx, entry) in info
        .entries
        .iter()
        .take(MAX_PREFETCH_ENTRY_THUMBS)
        .enumerate()
    {
        let thumb_url = entry.thumbnail.clone();
        let eid = entry.id.clone();
        let eurl = entry.effective_url();
        entry_tasks.push((
            idx,
            tokio::spawn(async move {
                vidgrab_core::fetch_thumbnail_with_fallback(thumb_url.as_deref(), &eid, &eurl).await
            }),
        ));
    }

    let top_thumb_url = info.thumbnail.clone();
    let top_id = info.id.clone();
    let top_url = info.webpage_url.clone().unwrap_or_else(|| url.clone());
    let top_data = if let Some(ref tu) = top_thumb_url {
        vidgrab_core::fetch_thumbnail_cached(&state.paths.data_dir.join("cache"), tu).await
    } else {
        vidgrab_core::fetch_thumbnail_with_fallback(None, &top_id, &top_url).await
    };

    for (idx, handle) in entry_tasks {
        if let Ok(Some(data_uri)) = handle.await {
            if let Some(entry) = info.entries.get_mut(idx) {
                entry.thumbnail = Some(data_uri);
            }
        }
    }

    if let Some(data_uri) = top_data {
        info.thumbnail = Some(data_uri);
    } else if info.thumbnail.is_none() {
        info.thumbnail = info.entries.first().and_then(|e| e.thumbnail.clone());
    }
    if !flat_playlist {
        let _ = state.db.put_cached_analysis(&url, &info);
    }
    push_app_log(
        &state.logs,
        Some(&app),
        format!(
            "Analysé : {} ({} format(s), {} entrée(s))",
            info.title,
            info.formats.len(),
            info.entries.len()
        ),
    );
    Ok(info)
}

#[tauri::command]
pub async fn cmd_paste_or_smart_download(
    state: State<'_, AppState>,
    url: String,
) -> CmdResult<Option<DownloadItem>> {
    let settings = state
        .settings
        .read()
        .map(|s| s.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;

    if settings.smart_mode.enabled {
        let info = engine::analyze_url(&state.paths, &settings, &url, false)
            .await
            .map_err(|e| e.to_string())?;
        let req = build_smart_download_request(&info, &settings);
        let item = state.queue.enqueue(req).map_err(|e| e.to_string())?;
        Ok(Some(item))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn cmd_enqueue_download(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DownloadRequest,
) -> CmdResult<DownloadItem> {
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Ajout file : {} [{}]", request.title, request.url),
    );
    state.queue.enqueue(request).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_import_bulk_file(
    state: State<'_, AppState>,
    file_path: PathBuf,
) -> CmdResult<Vec<String>> {
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;
    let urls = vidgrab_core::parse_bulk_urls(&content);
    let _ = &state.paths;
    Ok(urls)
}

#[tauri::command]
pub async fn cmd_list_downloads(state: State<'_, AppState>) -> CmdResult<Vec<DownloadItem>> {
    Ok(state.queue.list_items())
}

#[tauri::command]
pub async fn cmd_search_downloads(
    state: State<'_, AppState>,
    query: String,
) -> CmdResult<Vec<DownloadItem>> {
    state
        .db
        .search_downloads(&query)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_pause_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.pause(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_resume_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.resume(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_cancel_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.cancel(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_remove_download(
    state: State<'_, AppState>,
    id: Uuid,
    delete_file: bool,
) -> CmdResult<()> {
    state
        .queue
        .remove(id, delete_file)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_export_downloads_json(
    state: State<'_, AppState>,
    dest_path: PathBuf,
) -> CmdResult<()> {
    let json = state.db.export_json().map_err(|e| e.to_string())?;
    fs::write(&dest_path, json).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_import_downloads_json(
    state: State<'_, AppState>,
    src_path: PathBuf,
) -> CmdResult<(usize, usize)> {
    let json = fs::read_to_string(&src_path).map_err(|e| e.to_string())?;
    let counts = state.db.import_json(&json).map_err(|e| e.to_string())?;
    state.queue.reload_from_db().map_err(|e| e.to_string())?;
    Ok(counts)
}

#[tauri::command]
pub async fn cmd_pause_all(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.pause_all().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Pause globale ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
pub async fn cmd_resume_all(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.resume_all().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Reprise globale ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
pub async fn cmd_retry_failed(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.retry_failed().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Relance des échecs ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
pub async fn cmd_clear_finished(app: AppHandle, state: State<'_, AppState>) -> CmdResult<usize> {
    let n = state.queue.clear_finished().map_err(|e| e.to_string())?;
    push_app_log(
        &state.logs,
        Some(&app),
        format!("Nettoyage de la liste ({n} élément(s))"),
    );
    Ok(n)
}

#[tauri::command]
pub async fn cmd_prioritize_download(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.queue.prioritize(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_schedule_override_now(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.queue.set_schedule_bypass(true);
    push_app_log(
        &state.logs,
        Some(&app),
        "Planificateur horaire contourné manuellement",
    );
    Ok(())
}

#[tauri::command]
pub async fn cmd_get_schedule_status(state: State<'_, AppState>) -> CmdResult<bool> {
    let s = state
        .settings
        .read()
        .map(|g| g.clone())
        .map_err(|_| "Settings lock poisoned".to_string())?;
    let hour = chrono::Local::now().hour() as u8;
    Ok(s.schedule_enabled && !vidgrab_core::is_within_schedule(&s, hour))
}
