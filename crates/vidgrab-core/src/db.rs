//! Safe `rusqlite` persistence layer and versioned migrations for `vidgrab-core`.
//!
//! Stores the download history, active queue (for crash/restart recovery of `.part` files),
//! and channel/playlist subscriptions. Also supports full JSON export and import.

use crate::error::{CoreError, CoreResult};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;
use vidgrab_shared::{DownloadItem, DownloadStatus, ExportBundle, Subscription, VideoInfo};

const LATEST_SCHEMA_VERSION: i32 = 2;

fn url_cache_key(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.trim().as_bytes());
    hex::encode(hasher.finalize())
}

fn map_sql_err(err: rusqlite::Error) -> CoreError {
    CoreError::Database(err.to_string())
}

/// Thread-safe SQLite database handle for downloads and subscriptions.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens or creates the SQLite database at `db_path` and runs versioned schema migrations.
    pub fn open(db_path: &Path) -> CoreResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path).map_err(map_sql_err)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_sql_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(map_sql_err)?;

        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Returns the current SQLite `PRAGMA user_version`.
    pub fn schema_version(&self) -> CoreResult<i32> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        guard
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(map_sql_err)
    }

    fn migrate(&self) -> CoreResult<()> {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;

        let current_version: i32 = guard
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(map_sql_err)?;

        if current_version < 1 {
            let tx = guard.transaction().map_err(map_sql_err)?;
            tx.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS downloads (
                    id TEXT PRIMARY KEY NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    payload_json TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_downloads_created_at
                    ON downloads (created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_downloads_status
                    ON downloads (status);
                CREATE TABLE IF NOT EXISTS subscriptions (
                    id TEXT PRIMARY KEY NOT NULL,
                    url TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    payload_json TEXT NOT NULL
                );
                ",
            )
            .map_err(map_sql_err)?;
            tx.pragma_update(None, "user_version", 1)
                .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
        }

        if current_version < 2 {
            let tx = guard.transaction().map_err(map_sql_err)?;
            tx.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS analysis_cache (
                    url_hash TEXT PRIMARY KEY NOT NULL,
                    url TEXT NOT NULL,
                    info_json TEXT NOT NULL,
                    cached_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_analysis_cache_time
                    ON analysis_cache (cached_at);
                ",
            )
            .map_err(map_sql_err)?;
            let _ = tx.execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS downloads_fts USING fts5(id UNINDEXED, title, url, group_title);",
            );
            tx.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
                .map_err(map_sql_err)?;
            tx.commit().map_err(map_sql_err)?;
        }

        Ok(())
    }

    fn upsert_download_on_conn(conn: &Connection, item: &DownloadItem) -> CoreResult<()> {
        let id = item.request.id.to_string();
        let status = serde_json::to_string(&item.progress.status)?;
        let created_at = item.created_at.to_rfc3339();
        let payload = serde_json::to_string(item)?;

        conn.execute(
            "INSERT INTO downloads (id, status, created_at, payload_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 status = excluded.status,
                 payload_json = excluded.payload_json;",
            params![id, status, created_at, payload],
        )
        .map_err(map_sql_err)?;
        let _ = conn.execute("DELETE FROM downloads_fts WHERE id = ?1;", params![id]);
        let _ = conn.execute(
            "INSERT INTO downloads_fts (id, title, url, group_title) VALUES (?1, ?2, ?3, ?4);",
            params![
                id,
                item.request.title,
                item.request.url,
                item.request.group_title.as_deref().unwrap_or("")
            ],
        );
        Ok(())
    }

    fn upsert_download_in_tx(tx: &Transaction<'_>, item: &DownloadItem) -> CoreResult<()> {
        Self::upsert_download_on_conn(tx, item)
    }

    /// Inserts or updates a [`DownloadItem`] in the `downloads` table.
    pub fn upsert_download(&self, item: &DownloadItem) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::upsert_download_on_conn(&guard, item)
    }

    /// Removes a download item by ID.
    pub fn delete_download(&self, id: Uuid) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let id_str = id.to_string();
        guard
            .execute("DELETE FROM downloads WHERE id = ?1;", params![id_str])
            .map_err(map_sql_err)?;
        let _ = guard.execute("DELETE FROM downloads_fts WHERE id = ?1;", params![id_str]);
        Ok(())
    }

    /// Full-text search across download titles, URLs, and group titles (uses FTS5 when available).
    pub fn search_downloads(&self, query: &str) -> CoreResult<Vec<DownloadItem>> {
        let q = query.trim();
        if q.is_empty() {
            return self.list_downloads();
        }
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;

        let fts_query = q
            .split_whitespace()
            .map(|tok| format!("\"{}\"*", tok.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        let fts_attempt = guard.prepare(
            "SELECT d.payload_json FROM downloads d
             INNER JOIN downloads_fts f ON f.id = d.id
             WHERE downloads_fts MATCH ?1
             ORDER BY d.created_at DESC;",
        );

        let mut items = Vec::new();
        if let Ok(mut stmt) = fts_attempt {
            if let Ok(rows) = stmt.query_map(params![fts_query], |row| row.get::<_, String>(0)) {
                for row_res in rows {
                    let json = row_res.map_err(map_sql_err)?;
                    items.push(serde_json::from_str(&json)?);
                }
                return Ok(items);
            }
        }

        let like_pattern = format!("%{}%", q.to_lowercase());
        let mut stmt = guard
            .prepare(
                "SELECT payload_json FROM downloads
                 WHERE LOWER(payload_json) LIKE ?1
                 ORDER BY created_at DESC;",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![like_pattern], |row| row.get::<_, String>(0))
            .map_err(map_sql_err)?;
        for row_res in rows {
            let json = row_res.map_err(map_sql_err)?;
            items.push(serde_json::from_str(&json)?);
        }
        Ok(items)
    }

    /// Reads a cached [`VideoInfo`] analysis if present and younger than `max_age_secs`.
    pub fn get_cached_analysis(&self, url: &str, max_age_secs: i64) -> CoreResult<Option<VideoInfo>> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let key = url_cache_key(url);
        let min_ts = Utc::now().timestamp() - max_age_secs.max(0);
        let row: Option<String> = guard
            .query_row(
                "SELECT info_json FROM analysis_cache WHERE url_hash = ?1 AND cached_at >= ?2;",
                params![key, min_ts],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sql_err)?;
        match row {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Stores or refreshes a [`VideoInfo`] entry in `analysis_cache`.
    pub fn put_cached_analysis(&self, url: &str, info: &VideoInfo) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let key = url_cache_key(url);
        let json = serde_json::to_string(info)?;
        let now = Utc::now().timestamp();
        guard
            .execute(
                "INSERT INTO analysis_cache (url_hash, url, info_json, cached_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(url_hash) DO UPDATE SET
                     info_json = excluded.info_json,
                     cached_at = excluded.cached_at;",
                params![key, url, json, now],
            )
            .map_err(map_sql_err)?;
        Ok(())
    }

    /// Removes a batch of download items inside a single atomic SQLite transaction.
    pub fn delete_downloads_batch(&self, ids: &[Uuid]) -> CoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let tx = guard.transaction().map_err(map_sql_err)?;
        {
            let mut stmt = tx
                .prepare("DELETE FROM downloads WHERE id = ?1;")
                .map_err(map_sql_err)?;
            for id in ids {
                stmt.execute(params![id.to_string()]).map_err(map_sql_err)?;
            }
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    }

    /// Loads all download items ordered by `created_at` descending.
    pub fn list_downloads(&self) -> CoreResult<Vec<DownloadItem>> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let mut stmt = guard
            .prepare("SELECT payload_json FROM downloads ORDER BY created_at DESC;")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql_err)?;

        let mut items = Vec::new();
        for row_res in rows {
            let json = row_res.map_err(map_sql_err)?;
            let item: DownloadItem = serde_json::from_str(&json)?;
            items.push(item);
        }
        Ok(items)
    }

    /// Marks any download left in `Downloading` or `Processing` state from a previous crash/exit
    /// back into `Queued` so it can be resumed with `--continue` on `.part` files.
    pub fn recover_interrupted_downloads(&self) -> CoreResult<Vec<DownloadItem>> {
        let all = self.list_downloads()?;
        let mut to_recover: Vec<DownloadItem> = all
            .into_iter()
            .filter(|item| {
                matches!(
                    item.progress.status,
                    DownloadStatus::Downloading | DownloadStatus::Processing
                )
            })
            .collect();
        if to_recover.is_empty() {
            return Ok(to_recover);
        }

        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let tx = guard.transaction().map_err(map_sql_err)?;
        for item in &mut to_recover {
            item.progress.status = DownloadStatus::Queued;
            Self::upsert_download_in_tx(&tx, item)?;
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(to_recover)
    }

    fn upsert_subscription_on_conn(conn: &Connection, sub: &Subscription) -> CoreResult<()> {
        let id = sub.id.to_string();
        let url = sub.url.clone();
        let created_at = sub.created_at.to_rfc3339();
        let payload = serde_json::to_string(sub)?;

        conn.execute(
            "INSERT INTO subscriptions (id, url, created_at, payload_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 url = excluded.url,
                 payload_json = excluded.payload_json;",
            params![id, url, created_at, payload],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    /// Inserts or updates a [`Subscription`] in the `subscriptions` table.
    pub fn upsert_subscription(&self, sub: &Subscription) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::upsert_subscription_on_conn(&guard, sub)
    }

    /// Deletes a subscription by ID.
    pub fn delete_subscription(&self, id: Uuid) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        guard
            .execute(
                "DELETE FROM subscriptions WHERE id = ?1;",
                params![id.to_string()],
            )
            .map_err(map_sql_err)?;
        Ok(())
    }

    /// Lists all subscriptions ordered by `created_at` descending.
    pub fn list_subscriptions(&self) -> CoreResult<Vec<Subscription>> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let mut stmt = guard
            .prepare("SELECT payload_json FROM subscriptions ORDER BY created_at DESC;")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql_err)?;

        let mut subs = Vec::new();
        for row_res in rows {
            let json = row_res.map_err(map_sql_err)?;
            let sub: Subscription = serde_json::from_str(&json)?;
            subs.push(sub);
        }
        Ok(subs)
    }

    /// Exports all downloads and subscriptions into a single JSON string.
    pub fn export_json(&self) -> CoreResult<String> {
        let bundle = ExportBundle {
            version: 1,
            exported_at: Utc::now(),
            downloads: self.list_downloads()?,
            subscriptions: self.list_subscriptions()?,
        };
        let json = serde_json::to_string_pretty(&bundle)?;
        Ok(json)
    }

    /// Imports downloads and subscriptions from a JSON string exported by [`Self::export_json`]
    /// inside a single atomic SQLite transaction.
    pub fn import_json(&self, json_str: &str) -> CoreResult<(usize, usize)> {
        let bundle: ExportBundle = serde_json::from_str(json_str)?;
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let tx = guard.transaction().map_err(map_sql_err)?;
        for dl in &bundle.downloads {
            Self::upsert_download_in_tx(&tx, dl)?;
        }
        for sub in &bundle.subscriptions {
            Self::upsert_subscription_on_conn(&tx, sub)?;
        }
        tx.commit().map_err(map_sql_err)?;
        Ok((bundle.downloads.len(), bundle.subscriptions.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidgrab_shared::{DownloadProgress, DownloadRequest};

    #[test]
    fn test_db_schema_migration_and_export_import() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("vidgrab.db");
        let db = Database::open(&db_path).expect("open db");
        assert_eq!(db.schema_version().expect("version"), LATEST_SCHEMA_VERSION);

        let req = DownloadRequest::default();
        let item = DownloadItem {
            progress: DownloadProgress {
                id: req.id,
                ..Default::default()
            },
            request: req,
            created_at: Utc::now(),
            completed_at: None,
        };
        db.upsert_download(&item).expect("upsert");
        let exported = db.export_json().expect("export");
        assert!(exported.contains(&item.request.id.to_string()));

        let db2 = Database::open(&tmp.path().join("vidgrab2.db")).expect("open db2");
        let (dl_count, sub_count) = db2.import_json(&exported).expect("import");
        assert_eq!(dl_count, 1);
        assert_eq!(sub_count, 0);
        assert_eq!(db2.list_downloads().expect("list").len(), 1);
    }

    #[test]
    fn test_db_analysis_cache_and_fts5_search() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&tmp.path().join("cache.db")).expect("open db");

        let info = VideoInfo {
            id: "abc".to_string(),
            title: "Rust Async Masterclass".to_string(),
            ..Default::default()
        };
        db.put_cached_analysis("https://example.com/watch?v=abc", &info)
            .expect("put cache");
        let hit = db
            .get_cached_analysis("https://example.com/watch?v=abc", 900)
            .expect("get cache");
        assert_eq!(hit.map(|v| v.title), Some("Rust Async Masterclass".to_string()));

        let req = DownloadRequest {
            title: "Rust Async Masterclass".to_string(),
            url: "https://example.com/watch?v=abc".to_string(),
            ..Default::default()
        };
        let item = DownloadItem {
            progress: DownloadProgress {
                id: req.id,
                ..Default::default()
            },
            request: req,
            created_at: Utc::now(),
            completed_at: None,
        };
        db.upsert_download(&item).expect("upsert");
        let found = db.search_downloads("Masterclass").expect("search");
        assert_eq!(found.len(), 1);
        assert!(db.search_downloads("UnrelatedQueryXYZ").expect("search empty").is_empty());
    }
}
