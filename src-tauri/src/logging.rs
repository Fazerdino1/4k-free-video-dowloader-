//! In-memory ring-buffer logging and rotating file log helpers for Tauri.

use crate::AppState;
use chrono::Utc;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub fn push_app_log(
    logs: &Mutex<VecDeque<String>>,
    app: Option<&AppHandle>,
    msg: impl AsRef<str>,
) {
    let ts = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{ts}] {}", msg.as_ref());
    if let Ok(mut guard) = logs.lock() {
        if guard.len() >= 400 {
            guard.pop_front();
        }
        guard.push_back(line.clone());
    }
    if let Some(handle) = app {
        let _ = handle.emit("app-log", &line);
    }
}

/// Rotates and initializes log file in the logs directory.
pub fn init_rotating_log(logs_dir: &Path) {
    let _ = fs::create_dir_all(logs_dir);
    let log_file = logs_dir.join("vidgrab.log");
    if let Ok(meta) = fs::metadata(&log_file) {
        if meta.len() > 5 * 1024 * 1024 {
            let rotated = logs_dir.join("vidgrab.log.1");
            let _ = fs::rename(&log_file, rotated);
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_file) {
        let _ = writeln!(
            f,
            "[{}] 4k free video dowloader started",
            Utc::now().to_rfc3339()
        );
    }
}

#[tauri::command]
pub async fn cmd_get_logs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state
        .logs
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default())
}

#[tauri::command]
pub async fn cmd_clear_logs(state: State<'_, AppState>) -> Result<(), String> {
    if let Ok(mut g) = state.logs.lock() {
        g.clear();
    }
    Ok(())
}
