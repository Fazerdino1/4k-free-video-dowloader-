//! Asynchronous download queue manager with dynamic concurrency control,
//! SIGTERM pause/resume, partial file cleanup on cancellation, exponential-backoff retries,
//! and SQLite persistence.

use crate::config::AppPaths;
use crate::db::Database;
use crate::engine;
use crate::error::{CoreError, CoreResult};
use chrono::{Timelike, Utc};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tracing::{error, info, warn};
use uuid::Uuid;
use vidgrab_shared::{DownloadItem, DownloadProgress, DownloadRequest, DownloadStatus, Settings};

/// RAII guard that decrements active worker count and wakes waiting workers on drop.
struct WorkerGuard {
    active: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
        self.notify.notify_waiters();
    }
}

/// Manages active, queued, paused, and completed downloads.
#[derive(Clone)]
pub struct DownloadQueue {
    paths: AppPaths,
    db: Arc<Database>,
    settings: Arc<RwLock<Settings>>,
    items: Arc<RwLock<HashMap<Uuid, DownloadItem>>>,
    pids: Arc<RwLock<HashMap<Uuid, u32>>>,
    concurrency_limit: Arc<AtomicUsize>,
    active_workers: Arc<AtomicUsize>,
    concurrency_notify: Arc<Notify>,
    schedule_notify: Arc<Notify>,
    progress_tx: mpsc::UnboundedSender<DownloadProgress>,
    schedule_bypass: Arc<AtomicBool>,
}

impl DownloadQueue {
    /// Creates a new `DownloadQueue`, loading persisted items from SQLite and recovering
    /// any interrupted downloads from a previous session.
    pub fn new(
        paths: AppPaths,
        db: Arc<Database>,
        settings: Arc<RwLock<Settings>>,
        progress_tx: mpsc::UnboundedSender<DownloadProgress>,
    ) -> CoreResult<Self> {
        let _ = db.recover_interrupted_downloads()?;
        let existing = db.list_downloads()?;
        let mut map = HashMap::with_capacity(existing.len());
        for item in existing {
            map.insert(item.request.id, item);
        }

        let max_concurrent = settings
            .read()
            .map(|s| s.concurrent_downloads.clamp(1, 10))
            .unwrap_or(3);

        Ok(Self {
            paths,
            db,
            settings,
            items: Arc::new(RwLock::new(map)),
            pids: Arc::new(RwLock::new(HashMap::new())),
            concurrency_limit: Arc::new(AtomicUsize::new(max_concurrent)),
            active_workers: Arc::new(AtomicUsize::new(0)),
            concurrency_notify: Arc::new(Notify::new()),
            schedule_notify: Arc::new(Notify::new()),
            progress_tx,
            schedule_bypass: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Updates the maximum concurrent worker limit live without restarting active downloads.
    pub fn update_concurrency(&self, max_concurrent: usize) {
        self.concurrency_limit
            .store(max_concurrent.clamp(1, 10), Ordering::Release);
        self.concurrency_notify.notify_waiters();
    }

    /// Returns all download items sorted by priority (descending), then newest first.
    pub fn list_items(&self) -> Vec<DownloadItem> {
        let mut list: Vec<DownloadItem> = self
            .items
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|a| {
            (
                std::cmp::Reverse(a.request.priority),
                std::cmp::Reverse(a.created_at),
            )
        });
        list
    }

    /// Reloads any newly imported items from SQLite into the in-memory queue map.
    pub fn reload_from_db(&self) -> CoreResult<()> {
        let rows = self.db.list_downloads()?;
        if let Ok(mut guard) = self.items.write() {
            for item in rows {
                guard.entry(item.request.id).or_insert(item);
            }
        }
        Ok(())
    }

    /// Enqueues a [`DownloadRequest`], persists it to SQLite, and spawns its worker task.
    pub fn enqueue(&self, req: DownloadRequest) -> CoreResult<DownloadItem> {
        let id = req.id;
        let item = DownloadItem {
            request: req,
            progress: DownloadProgress {
                id,
                status: DownloadStatus::Queued,
                ..Default::default()
            },
            created_at: Utc::now(),
            completed_at: None,
        };

        self.db.upsert_download(&item)?;
        if let Ok(mut guard) = self.items.write() {
            guard.insert(id, item.clone());
        }
        let _ = self.progress_tx.send(item.progress.clone());

        self.spawn_worker(id, Vec::new());
        Ok(item)
    }

    /// Enqueues a download with extra `yt-dlp` arguments (used by Subscriptions with `--download-archive`).
    pub fn enqueue_with_extra_args(
        &self,
        req: DownloadRequest,
        extra_args: Vec<String>,
    ) -> CoreResult<DownloadItem> {
        let id = req.id;
        let item = DownloadItem {
            request: req,
            progress: DownloadProgress {
                id,
                status: DownloadStatus::Queued,
                ..Default::default()
            },
            created_at: Utc::now(),
            completed_at: None,
        };

        self.db.upsert_download(&item)?;
        if let Ok(mut guard) = self.items.write() {
            guard.insert(id, item.clone());
        }
        let _ = self.progress_tx.send(item.progress.clone());

        self.spawn_worker(id, extra_args);
        Ok(item)
    }

    /// Pauses an active download by sending `SIGTERM` to its `yt-dlp` process group, preserving `.part` files.
    pub fn pause(&self, id: Uuid) -> CoreResult<()> {
        let maybe_pid = self.pids.write().ok().and_then(|mut p| p.remove(&id));
        if let Some(pid) = maybe_pid {
            info!("Pausing download {} (PID {}) via SIGTERM", id, pid);
            let _ = engine::terminate_process(pid);
        }
        crate::mobile_bridge::cancel_task_if_registered(id);
        let res = self.update_status(id, DownloadStatus::Paused, None);
        self.concurrency_notify.notify_waiters();
        self.schedule_notify.notify_waiters();
        res
    }

    /// Resumes a paused or failed download using `yt-dlp --continue` on the existing `.part` file.
    pub fn resume(&self, id: Uuid) -> CoreResult<()> {
        if let Ok(mut guard) = self.items.write() {
            if let Some(item) = guard.get_mut(&id) {
                item.progress.retry_count = 0;
                item.progress.max_retries = 0;
            }
        }
        self.update_status(id, DownloadStatus::Queued, None)?;
        self.spawn_worker(id, Vec::new());
        Ok(())
    }

    /// Cancels a download, terminating `yt-dlp` if running and deleting partial `.part` files.
    pub fn cancel(&self, id: Uuid) -> CoreResult<()> {
        let maybe_pid = self.pids.write().ok().and_then(|mut p| p.remove(&id));
        if let Some(pid) = maybe_pid {
            info!("Cancelling download {} (PID {})", id, pid);
            let _ = engine::terminate_process(pid);
        }
        crate::mobile_bridge::cancel_task_if_registered(id);

        let maybe_req = self
            .items
            .read()
            .ok()
            .and_then(|m| m.get(&id).map(|i| i.request.clone()));

        if let Some(req) = maybe_req {
            let _ = engine::cleanup_partial_files(&req.output_dir, &req.title);
        }

        let res = self.update_status(id, DownloadStatus::Cancelled, None);
        self.concurrency_notify.notify_waiters();
        self.schedule_notify.notify_waiters();
        res
    }

    /// Removes an item from the queue and database. If `delete_file` is true, also deletes
    /// the downloaded file from disk.
    pub fn remove(&self, id: Uuid, delete_file: bool) -> CoreResult<()> {
        let _ = self.cancel(id);
        let removed = self.items.write().ok().and_then(|mut m| m.remove(&id));
        if delete_file {
            if let Some(item) = removed {
                if let Some(path) = item.progress.output_path {
                    if path.exists() {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
        self.db.delete_download(id)
    }

    /// Pauses all active or queued downloads, returning the number of items affected.
    pub fn pause_all(&self) -> CoreResult<usize> {
        let ids: Vec<Uuid> = self
            .list_items()
            .into_iter()
            .filter(|i| {
                matches!(
                    i.progress.status,
                    DownloadStatus::Queued
                        | DownloadStatus::Downloading
                        | DownloadStatus::Processing
                )
            })
            .map(|i| i.request.id)
            .collect();
        let count = ids.len();
        for id in ids {
            let _ = self.pause(id);
        }
        Ok(count)
    }

    /// Resumes all paused downloads.
    pub fn resume_all(&self) -> CoreResult<usize> {
        let ids: Vec<Uuid> = self
            .list_items()
            .into_iter()
            .filter(|i| i.progress.status == DownloadStatus::Paused)
            .map(|i| i.request.id)
            .collect();
        let count = ids.len();
        for id in ids {
            let _ = self.resume(id);
        }
        Ok(count)
    }

    /// Retries all failed or cancelled downloads.
    pub fn retry_failed(&self) -> CoreResult<usize> {
        let ids: Vec<Uuid> = self
            .list_items()
            .into_iter()
            .filter(|i| {
                matches!(
                    i.progress.status,
                    DownloadStatus::Failed | DownloadStatus::Cancelled
                )
            })
            .map(|i| i.request.id)
            .collect();
        let count = ids.len();
        for id in ids {
            let _ = self.resume(id);
        }
        Ok(count)
    }

    /// Clears all completed or cancelled items from the queue list without deleting files on disk.
    pub fn clear_finished(&self) -> CoreResult<usize> {
        let ids: Vec<Uuid> = self
            .list_items()
            .into_iter()
            .filter(|i| {
                matches!(
                    i.progress.status,
                    DownloadStatus::Completed | DownloadStatus::Cancelled
                )
            })
            .map(|i| i.request.id)
            .collect();
        let count = ids.len();
        if let Ok(mut guard) = self.items.write() {
            for id in &ids {
                guard.remove(id);
            }
        }
        self.db.delete_downloads_batch(&ids)?;
        Ok(count)
    }

    /// Raises `id`'s priority so it sits at the top of the queue.
    pub fn prioritize(&self, id: Uuid) -> CoreResult<()> {
        let mut updated = None;
        if let Ok(mut guard) = self.items.write() {
            let top = guard
                .values()
                .map(|i| i.request.priority)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            if let Some(item) = guard.get_mut(&id) {
                item.request.priority = top;
                updated = Some(item.clone());
            }
        }
        if let Some(item) = updated {
            self.db.upsert_download(&item)?;
            Ok(())
        } else {
            Err(CoreError::Database(format!("Download item {id} not found")))
        }
    }

    /// Temporarily bypasses the night schedule restriction so queued items can run immediately.
    pub fn set_schedule_bypass(&self, bypass: bool) {
        self.schedule_bypass.store(bypass, Ordering::Relaxed);
        if bypass {
            self.schedule_notify.notify_waiters();
        }
    }

    /// Returns `true` if any item is still queued, downloading, or processing.
    pub fn has_active_or_queued(&self) -> bool {
        self.items
            .read()
            .map(|m| {
                m.values().any(|i| {
                    matches!(
                        i.progress.status,
                        DownloadStatus::Queued
                            | DownloadStatus::Downloading
                            | DownloadStatus::Processing
                    )
                })
            })
            .unwrap_or(false)
    }

    fn maybe_write_group_m3u(&self, req: &DownloadRequest) {
        if !req.generate_m3u {
            return;
        }
        let mut entries: Vec<(Option<u32>, Option<String>, std::path::PathBuf)> = Vec::new();
        if let Ok(guard) = self.items.read() {
            for item in guard.values() {
                let same_group = match (req.group_id, item.request.group_id) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => item.request.id == req.id,
                    _ => false,
                };
                if same_group && item.progress.status == DownloadStatus::Completed {
                    if let Some(path) = &item.progress.output_path {
                        entries.push((
                            item.request.playlist_index,
                            Some(item.request.title.clone()),
                            path.clone(),
                        ));
                    }
                }
            }
        }
        if entries.is_empty() {
            return;
        }
        entries.sort_by_key(|(idx, _, _)| idx.unwrap_or(u32::MAX));
        let title = req
            .group_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&req.title);
        let pairs: Vec<(Option<String>, std::path::PathBuf)> =
            entries.into_iter().map(|(_, t, p)| (t, p)).collect();
        let m3u_text = engine::parser::generate_local_m3u_content(title, &pairs);

        let mut dir = req.output_dir.clone();
        if let Some(sub) = &req.subfolder_name {
            let san = engine::args::sanitize_folder_name(sub);
            if !san.is_empty() {
                dir = dir.join(san);
            }
        }
        let file_name = format!(
            "{}.m3u",
            engine::args::sanitize_folder_name(title).trim_end_matches('.')
        );
        let m3u_path = dir.join(if file_name == ".m3u" {
            "playlist.m3u".to_string()
        } else {
            file_name
        });
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(m3u_path, m3u_text);
    }

    fn update_status(
        &self,
        id: Uuid,
        status: DownloadStatus,
        error_message: Option<String>,
    ) -> CoreResult<()> {
        let mut updated_item = None;
        if let Ok(mut guard) = self.items.write() {
            if let Some(item) = guard.get_mut(&id) {
                item.progress.status = status;
                item.progress.error_message = error_message;
                if status == DownloadStatus::Completed {
                    item.completed_at = Some(Utc::now());
                }
                updated_item = Some(item.clone());
            }
        }
        if let Some(item) = updated_item {
            self.db.upsert_download(&item)?;
            let _ = self.progress_tx.send(item.progress);
            Ok(())
        } else {
            Err(CoreError::Database(format!("Download item {id} not found")))
        }
    }

    fn is_queued(&self, id: Uuid) -> bool {
        self.items
            .read()
            .ok()
            .and_then(|g| g.get(&id).map(|i| i.progress.status))
            .map(|s| s == DownloadStatus::Queued)
            .unwrap_or(false)
    }

    fn is_interrupted(&self, id: Uuid) -> bool {
        self.items
            .read()
            .ok()
            .and_then(|g| g.get(&id).map(|i| i.progress.status))
            .map(|s| matches!(s, DownloadStatus::Paused | DownloadStatus::Cancelled))
            .unwrap_or(false)
    }

    async fn wait_for_schedule(&self, id: Uuid) -> bool {
        loop {
            let notified = self.schedule_notify.notified();
            if !self.is_queued(id) {
                return false;
            }
            let s = self.settings.read().map(|g| g.clone()).unwrap_or_default();
            let hour = chrono::Local::now().hour() as u8;
            if self.schedule_bypass.load(Ordering::Relaxed) || crate::is_within_schedule(&s, hour) {
                return true;
            }
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
    }

    async fn acquire_worker_slot(&self, id: Uuid) -> Option<WorkerGuard> {
        loop {
            let notified = self.concurrency_notify.notified();
            if !self.is_queued(id) {
                return None;
            }
            let limit = self.concurrency_limit.load(Ordering::Acquire).clamp(1, 10);
            let mut cur = self.active_workers.load(Ordering::Acquire);
            while cur < limit {
                match self.active_workers.compare_exchange_weak(
                    cur,
                    cur + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        return Some(WorkerGuard {
                            active: self.active_workers.clone(),
                            notify: self.concurrency_notify.clone(),
                        });
                    }
                    Err(actual) => cur = actual,
                }
            }
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    async fn wait_backoff_interruptibly(&self, id: Uuid, delay_secs: u64) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(delay_secs);
        loop {
            let notified = self.schedule_notify.notified();
            if !self.is_queued(id) {
                return false;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return true;
            }
            let remaining = deadline.saturating_duration_since(now);
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(remaining) => return self.is_queued(id),
            }
        }
    }

    fn queued_request_snapshot(&self, id: Uuid) -> Option<(DownloadRequest, (u8, u8))> {
        let guard = self.items.read().ok()?;
        match guard.get(&id) {
            Some(item) if item.progress.status == DownloadStatus::Queued => Some((
                item.request.clone(),
                (item.progress.retry_count, item.progress.max_retries),
            )),
            _ => None,
        }
    }

    async fn enrich_request_metadata(
        &self,
        id: Uuid,
        req: &mut DownloadRequest,
        current_settings: &Settings,
    ) {
        let need_channel =
            current_settings.auto_organize_by_channel && req.subfolder_name.is_none();
        if req.thumbnail.is_none()
            || req.title == req.url
            || req.title == "Untitled"
            || need_channel
        {
            if let Ok(info) =
                engine::analyze_url(&self.paths, current_settings, &req.url, false).await
            {
                if !info.title.is_empty() && (req.title == req.url || req.title == "Untitled") {
                    req.title = info.title;
                }
                if req.thumbnail.is_none() {
                    req.thumbnail = info.thumbnail;
                }
                if need_channel {
                    if let Some(ch) = info.uploader.or(info.channel) {
                        let sanitized = engine::args::sanitize_folder_name(&ch);
                        if !sanitized.is_empty() {
                            req.subfolder_name = Some(sanitized);
                        }
                    }
                }
            }
        }
        if req
            .thumbnail
            .as_deref()
            .is_none_or(|t| !t.starts_with("data:image/"))
        {
            let cur_thumb = req.thumbnail.clone();
            if let Some(data_uri) =
                crate::fetch_thumbnail_with_fallback(cur_thumb.as_deref(), "", &req.url).await
            {
                req.thumbnail = Some(data_uri);
            }
        }
        if let Ok(mut guard) = self.items.write() {
            if let Some(item) = guard.get_mut(&id) {
                item.request.title = req.title.clone();
                item.request.thumbnail = req.thumbnail.clone();
                item.request.subfolder_name = req.subfolder_name.clone();
                let _ = self.db.upsert_download(item);
                let _ = self.progress_tx.send(item.progress.clone());
            }
        }
    }

    async fn run_attempt(
        &self,
        id: Uuid,
        req: &DownloadRequest,
        current_settings: &Settings,
        extra_args: &[String],
        retry_meta: (u8, u8),
    ) -> CoreResult<DownloadProgress> {
        let (inner_tx, mut inner_rx) = mpsc::unbounded_channel::<DownloadProgress>();
        let queue_for_progress = self.clone();

        let forward_handle = tokio::spawn(async move {
            while let Some(mut prog) = inner_rx.recv().await {
                prog.retry_count = retry_meta.0;
                prog.max_retries = retry_meta.1;
                if let Ok(mut guard) = queue_for_progress.items.write() {
                    if let Some(item) = guard.get_mut(&id) {
                        if matches!(
                            item.progress.status,
                            DownloadStatus::Paused | DownloadStatus::Cancelled
                        ) {
                            continue;
                        }
                        item.progress = prog.clone();
                    }
                }
                let _ = queue_for_progress.progress_tx.send(prog);
            }
        });

        let pids_ref = self.pids.clone();
        let result = engine::run_download(
            &self.paths,
            req,
            current_settings,
            extra_args,
            inner_tx,
            move |pid| {
                if let Ok(mut p) = pids_ref.write() {
                    p.insert(id, pid);
                }
            },
        )
        .await;

        if let Ok(mut p) = self.pids.write() {
            p.remove(&id);
        }
        let _ = forward_handle.await;
        result
    }

    fn mark_retrying(&self, id: Uuid, retry_count: u8, max_retries: u8, msg: String) {
        if let Ok(mut guard) = self.items.write() {
            if let Some(item) = guard.get_mut(&id) {
                item.progress.status = DownloadStatus::Queued;
                item.progress.retry_count = retry_count;
                item.progress.max_retries = max_retries;
                item.progress.error_message = Some(msg);
                item.progress.speed_bps = None;
                item.progress.eta_seconds = None;
                let _ = self.db.upsert_download(item);
                let _ = self.progress_tx.send(item.progress.clone());
            }
        }
    }

    fn finish_completed(&self, id: Uuid, req: &DownloadRequest, final_prog: DownloadProgress) {
        if let Ok(mut guard) = self.items.write() {
            if let Some(item) = guard.get_mut(&id) {
                item.progress = final_prog;
                item.progress.status = DownloadStatus::Completed;
                item.progress.error_message = None;
                item.completed_at = Some(Utc::now());
                let _ = self.db.upsert_download(item);
                let _ = self.progress_tx.send(item.progress.clone());
            }
        }
        self.maybe_write_group_m3u(req);
    }

    fn spawn_worker(&self, id: Uuid, extra_args: Vec<String>) {
        let queue = self.clone();
        tokio::spawn(async move {
            let mut attempt: u8 = 0;
            loop {
                if !queue.wait_for_schedule(id).await {
                    return;
                }

                let Some(_permit) = queue.acquire_worker_slot(id).await else {
                    return;
                };

                let Some((mut req, retry_meta)) = queue.queued_request_snapshot(id) else {
                    return;
                };

                let current_settings = queue.settings.read().map(|s| s.clone()).unwrap_or_default();
                let max_retries = current_settings.max_auto_retries.min(5);
                queue
                    .enrich_request_metadata(id, &mut req, &current_settings)
                    .await;
                let _ = queue.update_status(id, DownloadStatus::Downloading, None);

                let result = queue
                    .run_attempt(id, &req, &current_settings, &extra_args, retry_meta)
                    .await;
                drop(_permit);

                if queue.is_interrupted(id) {
                    return;
                }

                match result {
                    Ok(final_prog) => {
                        queue.finish_completed(id, &req, final_prog);
                        return;
                    }
                    Err(err) if attempt < max_retries && err.is_transient() => {
                        attempt = attempt.saturating_add(1);
                        let delay_secs = 2u64.pow((attempt - 1) as u32) * 2;
                        let msg = format!(
                            "Tentative {attempt}/{max_retries} dans {delay_secs} s ({err})"
                        );
                        warn!(
                            "Download {} transient failure: {}; retrying in {}s",
                            id, err, delay_secs
                        );
                        queue.mark_retrying(id, attempt, max_retries, msg);
                        if !queue.wait_backoff_interruptibly(id, delay_secs).await {
                            return;
                        }
                    }
                    Err(err) => {
                        error!("Download {} failed: {}", id, err);
                        let _ =
                            queue.update_status(id, DownloadStatus::Failed, Some(err.to_string()));
                        return;
                    }
                }
            }
        });
    }
}
