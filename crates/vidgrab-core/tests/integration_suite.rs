//! Integration test suite verifying the interaction between `vidgrab-core`,
//! `vidgrab-shared`, and `vidgrab-ui`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use uuid::Uuid;
use vidgrab_core::ai_audio::build_ffmpeg_audio_filter;
use vidgrab_core::config::{load_or_create_settings, save_settings, AppPaths};
use vidgrab_core::cookies::{parse_netscape_cookies, write_netscape_cookies_file};
use vidgrab_core::db::Database;
use vidgrab_core::engine::args::{build_analyze_args, build_args, build_subscription_archive_args};
use vidgrab_core::engine::parser::{generate_m3u_content, parse_progress_line, parse_video_info};
use vidgrab_core::error::CoreError;
use vidgrab_core::queue::DownloadQueue;
use vidgrab_core::smart_mode::build_smart_download_request;
use vidgrab_core::subscriptions::SubscriptionManager;
use vidgrab_shared::*;
use vidgrab_ui::pages::render_app_shell;

#[tokio::test]
async fn integration_full_workflow() {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
    let paths = AppPaths::from_roots(&tmp.path().join("config"), &tmp.path().join("data"))
        .unwrap_or_else(|e| panic!("AppPaths failed: {e}"));

    // 1. Settings creation & persistence
    let mut settings =
        load_or_create_settings(&paths).unwrap_or_else(|e| panic!("load settings: {e}"));
    settings.smart_mode.enabled = true;
    settings.smart_mode.target_height = Some(1440);
    settings.proxy.enabled = true;
    settings.proxy.url = "http://127.0.0.1:8888".to_string();
    settings.proxy.only_domains = vec!["bilibili.com".to_string()];
    save_settings(&paths, &settings).unwrap_or_else(|e| panic!("save settings: {e}"));

    // 2. Domain-scoped proxy argument verification
    let yt_args = build_analyze_args("https://www.youtube.com/watch?v=123", false, &settings);
    assert!(!yt_args.contains(&"--proxy".to_string()));
    let bili_args = build_analyze_args("https://www.bilibili.com/video/BV1xx", false, &settings);
    assert!(bili_args.contains(&"--proxy".to_string()));

    // 3. Playlist parsing & M3U generation
    let playlist_json = r#"{
        "id": "PL12345",
        "title": "Rust Tutorials",
        "_type": "playlist",
        "entries": [
            {"id": "v1", "title": "Episode 1", "duration": 300.0, "url": "https://www.youtube.com/watch?v=v1"},
            {"id": "v2", "title": "Episode 2", "duration": 450.0, "url": "https://www.youtube.com/watch?v=v2"}
        ]
    }"#;
    let pl_info = parse_video_info(playlist_json).unwrap_or_else(|e| panic!("playlist parse: {e}"));
    assert!(pl_info.is_playlist());
    assert_eq!(pl_info.entries.len(), 2);
    let m3u = generate_m3u_content(&pl_info);
    assert!(m3u.contains("#EXTINF:300,Episode 1"));

    // 4. Smart Mode fallback & queue persistence
    let single_json = r#"{
        "id": "vid999",
        "title": "4K Demo",
        "webpage_url": "https://www.youtube.com/watch?v=vid999",
        "formats": [
            {"format_id": "271", "height": 1080, "vcodec": "vp9", "acodec": "none"}
        ]
    }"#;
    let v_info = parse_video_info(single_json).unwrap_or_else(|e| panic!("single parse: {e}"));
    let smart_req = build_smart_download_request(&v_info, &settings);
    assert_eq!(smart_req.target_height, Some(1080)); // 1440 requested -> falls back to 1080

    let cmd_args = build_args(&smart_req, &settings);
    assert!(cmd_args.contains(&"--continue".to_string()));

    // 5. Database, Queue, Subscriptions, and UI rendering
    let db = Arc::new(Database::open(&paths.db_path).unwrap_or_else(|e| panic!("db open: {e}")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let shared_settings = Arc::new(RwLock::new(settings.clone()));
    let queue = DownloadQueue::new(paths.clone(), db.clone(), shared_settings.clone(), tx)
        .unwrap_or_else(|e| panic!("queue: {e}"));

    let sub_mgr =
        SubscriptionManager::new(paths.clone(), db.clone(), queue.clone(), shared_settings);
    let sub_id = Uuid::new_v4();
    let archive_args = build_subscription_archive_args(&sub_mgr.archive_file_path(sub_id), 10);
    assert!(archive_args.contains(&"--download-archive".to_string()));

    // 6. Netscape cookies & AI audio filters
    let cookies = vec![CookieEntry {
        domain: ".youtube.com".to_string(),
        include_subdomains: true,
        path: "/".to_string(),
        secure: true,
        expires: 2_000_000_000,
        name: "LOGIN_INFO".to_string(),
        value: "xyz".to_string(),
    }];
    write_netscape_cookies_file(&paths.cookies_file, &cookies)
        .unwrap_or_else(|e| panic!("cookies write: {e}"));
    let raw_cookies = std::fs::read_to_string(&paths.cookies_file)
        .unwrap_or_else(|e| panic!("read cookies: {e}"));
    assert_eq!(parse_netscape_cookies(&raw_cookies).len(), 1);

    assert!(build_ffmpeg_audio_filter(AiAudioOperation::ReduceEchoReverb).contains("afftdn"));
    assert!(matches!(
        CoreError::from_yt_dlp_stderr("ERROR: [youtube] xyz: This video is private"),
        CoreError::PrivateVideo
    ));
    assert!(matches!(
        CoreError::from_yt_dlp_stderr("ERROR: Sign in to confirm your age"),
        CoreError::AgeRestricted
    ));

    let prog = parse_progress_line(
        Uuid::nil(),
        r#"download:{"status":"finished","downloaded_bytes":1000,"total_bytes":1000,"total_bytes_estimate":1000,"speed":500.0,"eta":0,"fragment_index":1,"fragment_count":1,"filename":"/tmp/out.mkv"}"#,
    );
    assert!(prog.is_some());
    assert_eq!(
        prog.and_then(|p| p.output_path),
        Some(PathBuf::from("/tmp/out.mkv"))
    );

    assert!(settings.notifications_enabled);

    let html = render_app_shell(&queue.list_items(), &settings);
    assert!(html.contains("4k free video dowloader"));
    assert!(html.contains("btn-toggle-notifications"));
}
