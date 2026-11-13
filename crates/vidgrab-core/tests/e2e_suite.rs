//! End-to-End (E2E) test suite with a local mock `yt-dlp` binary.
//!
//! Exercises full `DownloadQueue` execution, progress updates, SQLite v2 FTS5 indexing,
//! and completion without requiring internet access.

use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use vidgrab_core::config::AppPaths;
use vidgrab_core::db::Database;
use vidgrab_core::queue::DownloadQueue;
use vidgrab_shared::{DownloadProgress, DownloadRequest, DownloadStatus, Settings};

#[cfg(unix)]
#[tokio::test]
async fn test_e2e_queue_with_mock_ytdlp() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::from_roots(tmp.path(), tmp.path()).expect("paths");

    let mock_ytdlp = tmp.path().join("mock-yt-dlp");
    let out_dir = tmp.path().join("downloads");
    std::fs::create_dir_all(&out_dir).expect("mkdir");

    let script = format!(
        r#"#!/usr/bin/env sh
if [ "$1" = "--version" ]; then
  echo "2026.11.12"
  exit 0
fi
OUT="{}/e2e_sample.mp4"
printf "MOCK_MP4_DATA" > "$OUT"
echo '{{"status":"downloading","downloaded_bytes":50,"total_bytes":100,"speed":50.0,"eta":1,"filename":"'"$OUT"'"}}'
echo '{{"status":"finished","downloaded_bytes":100,"total_bytes":100,"filename":"'"$OUT"'"}}'
exit 0
"#,
        out_dir.display()
    );
    std::fs::write(&mock_ytdlp, script).expect("write mock");
    let mut perms = std::fs::metadata(&mock_ytdlp).expect("meta").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&mock_ytdlp, perms).expect("chmod");

    let settings = Settings {
        custom_yt_dlp_path: Some(mock_ytdlp),
        video_output_dir: out_dir.clone(),
        ..Default::default()
    };

    let db = Arc::new(Database::open(&paths.db_path).expect("open db"));
    let shared_settings = Arc::new(RwLock::new(settings));
    let (tx, mut rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let queue = DownloadQueue::new(paths, db.clone(), shared_settings, tx).expect("queue");

    let req = DownloadRequest {
        url: "https://example.com/e2e-video".to_string(),
        title: "E2E Mocked Video".to_string(),
        output_dir: out_dir,
        ..Default::default()
    };

    let item = queue.enqueue(req).expect("enqueue");
    let id = item.request.id;

    let mut completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Some(prog)) if prog.id == id && prog.status == DownloadStatus::Completed => {
                completed = true;
                break;
            }
            Ok(Some(prog)) if prog.id == id && prog.status == DownloadStatus::Failed => {
                panic!("Mock download failed unexpectedly: {:?}", prog.error_message);
            }
            _ => {}
        }
    }

    assert!(completed, "Download did not reach Completed state in time");
    let fts_hits = db.search_downloads("Mocked").expect("fts5 search");
    assert_eq!(fts_hits.len(), 1);
    assert_eq!(fts_hits[0].request.id, id);
}
