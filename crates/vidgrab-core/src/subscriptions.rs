//! Channel and playlist subscription manager.
//!
//! Periodically polls subscribed URLs using `tokio::time::interval`, filters out
//! already archived video IDs via `--download-archive`, and automatically queues
//! newly published videos with the subscription's quality and format settings.

use crate::config::AppPaths;
use crate::db::Database;
use crate::engine;
use crate::error::CoreResult;
use crate::queue::DownloadQueue;
use chrono::Utc;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;
use vidgrab_shared::{DownloadItem, DownloadRequest, Settings, Subscription};

/// Manages channel/playlist subscriptions and periodic background checks.
#[derive(Clone)]
pub struct SubscriptionManager {
    paths: AppPaths,
    db: Arc<Database>,
    queue: DownloadQueue,
    settings: Arc<RwLock<Settings>>,
}

impl SubscriptionManager {
    pub fn new(
        paths: AppPaths,
        db: Arc<Database>,
        queue: DownloadQueue,
        settings: Arc<RwLock<Settings>>,
    ) -> Self {
        Self {
            paths,
            db,
            queue,
            settings,
        }
    }

    /// Returns the path to the `--download-archive` file for a given subscription ID.
    pub fn archive_file_path(&self, sub_id: Uuid) -> PathBuf {
        self.paths.archive_dir.join(format!("sub_{sub_id}.archive"))
    }

    /// Reads the set of already-downloaded video IDs from the subscription's archive file.
    pub fn read_archived_ids(&self, sub_id: Uuid) -> HashSet<String> {
        let path = self.archive_file_path(sub_id);
        let Ok(content) = fs::read_to_string(path) else {
            return HashSet::new();
        };
        content
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let _extractor = parts.next()?;
                let id = parts.next()?;
                Some(id.to_string())
            })
            .collect()
    }

    /// Appends a video ID to the subscription's archive file in `yt-dlp` format (`youtube <id>`).
    pub fn record_archived_id(
        &self,
        sub_id: Uuid,
        extractor: &str,
        video_id: &str,
    ) -> CoreResult<()> {
        let path = self.archive_file_path(sub_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{} {}", extractor.to_lowercase(), video_id)?;
        Ok(())
    }

    /// Adds or updates a subscription in SQLite.
    pub fn save_subscription(&self, sub: &Subscription) -> CoreResult<()> {
        self.db.upsert_subscription(sub)
    }

    /// Deletes a subscription and its archive file.
    pub fn remove_subscription(&self, id: Uuid) -> CoreResult<()> {
        let archive = self.archive_file_path(id);
        if archive.exists() {
            let _ = fs::remove_file(archive);
        }
        self.db.delete_subscription(id)
    }

    /// Lists all subscriptions.
    pub fn list_subscriptions(&self) -> CoreResult<Vec<Subscription>> {
        self.db.list_subscriptions()
    }

    /// Checks a single subscription now, enqueueing any new videos not yet in its archive.
    /// Returns the list of newly enqueued [`DownloadItem`]s.
    pub async fn check_subscription_now(&self, sub_id: Uuid) -> CoreResult<Vec<DownloadItem>> {
        let subs = self.db.list_subscriptions()?;
        let Some(mut sub) = subs.into_iter().find(|s| s.id == sub_id) else {
            return Ok(Vec::new());
        };

        let settings = self.settings.read().map(|s| s.clone()).unwrap_or_default();

        info!("Checking subscription '{}' ({})", sub.title, sub.url);
        let info = engine::analyze_url(&self.paths, &settings, &sub.url, true).await?;

        let archived = self.read_archived_ids(sub.id);
        let archive_path = self.archive_file_path(sub.id);
        let extra_args =
            engine::args::build_subscription_archive_args(&archive_path, sub.max_videos);

        let limit = if sub.max_videos == 0 {
            usize::MAX
        } else {
            sub.max_videos as usize
        };

        let mut enqueued = Vec::new();
        for entry in info.entries.iter().take(limit) {
            if entry.id.is_empty() || archived.contains(&entry.id) {
                continue;
            }
            let entry_url = entry.effective_url();
            if !sub.include_shorts && entry_url.contains("/shorts/") {
                continue;
            }
            if !sub.include_lives && entry.title.to_lowercase().contains("[live]") {
                continue;
            }

            let req = DownloadRequest {
                id: Uuid::new_v4(),
                url: entry_url,
                title: entry.title.clone(),
                thumbnail: entry.thumbnail.clone().or_else(|| sub.thumbnail.clone()),
                group_id: Some(sub.id),
                group_title: Some(sub.title.clone()),
                media_type: sub.media_type,
                target_height: sub.target_height,
                video_container: sub.video_container,
                audio_format: sub.audio_format,
                output_dir: sub.output_dir.clone(),
                filename_template: settings.filename_template.clone(),
                subfolder_name: Some(sub.title.clone()),
                ..Default::default()
            };

            let item = self
                .queue
                .enqueue_with_extra_args(req, extra_args.clone())?;
            let extractor = info.extractor_key.as_deref().unwrap_or("youtube");
            let _ = self.record_archived_id(sub.id, extractor, &entry.id);
            enqueued.push(item);
        }

        sub.last_checked_at = Some(Utc::now());
        self.db.upsert_subscription(&sub)?;
        Ok(enqueued)
    }

    /// Starts the background `tokio::time::interval` loop that checks due subscriptions.
    pub fn spawn_scheduler<F>(&self, on_new_videos: F)
    where
        F: Fn(Subscription, Vec<DownloadItem>) + Send + Sync + 'static,
    {
        let mgr = self.clone();
        let callback = Arc::new(on_new_videos);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let Ok(subs) = mgr.list_subscriptions() else {
                    continue;
                };
                let now = Utc::now();
                for sub in subs {
                    if sub.paused {
                        continue;
                    }
                    let interval_mins = sub.check_interval_mins.clamp(15, 1440) as i64;
                    let is_due = match sub.last_checked_at {
                        None => true,
                        Some(last) => (now - last).num_minutes() >= interval_mins,
                    };
                    if is_due {
                        match mgr.check_subscription_now(sub.id).await {
                            Ok(items) if !items.is_empty() => {
                                callback(sub, items);
                            }
                            Ok(_) => {}
                            Err(e) => {
                                warn!("Subscription check failed for {}: {}", sub.title, e);
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidgrab_shared::{AudioFormat, MediaType, VideoContainer};

    #[tokio::test]
    async fn test_subscription_archive_tracking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::from_roots(tmp.path(), tmp.path()).expect("paths");
        let db = Arc::new(Database::open(&paths.db_path).expect("open db"));
        let settings = Arc::new(RwLock::new(Settings::default()));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let queue = DownloadQueue::new(paths.clone(), db.clone(), settings.clone(), tx)
            .expect("queue");
        let mgr = SubscriptionManager::new(paths, db, queue, settings);

        let sub = Subscription {
            id: Uuid::new_v4(),
            url: "https://example.com/@channel".to_string(),
            title: "Test Channel".to_string(),
            thumbnail: None,
            check_interval_mins: 60,
            media_type: MediaType::Video,
            target_height: Some(1080),
            video_container: VideoContainer::Mp4,
            audio_format: AudioFormat::Mp3,
            output_dir: tmp.path().to_path_buf(),
            max_videos: 10,
            include_shorts: true,
            include_lives: false,
            paused: false,
            last_checked_at: None,
            created_at: Utc::now(),
        };
        mgr.save_subscription(&sub).expect("save sub");
        assert_eq!(mgr.list_subscriptions().expect("list").len(), 1);
        mgr.record_archived_id(sub.id, "youtube", "vid123")
            .expect("archive");
        let archived = mgr.read_archived_ids(sub.id);
        assert!(archived.contains("vid123"));
        assert!(!archived.contains("vid999"));
        mgr.remove_subscription(sub.id).expect("remove sub");
        assert!(mgr.list_subscriptions().expect("list").is_empty());
    }
}
