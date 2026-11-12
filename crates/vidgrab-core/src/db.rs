//! Native SQLite3 FFI bindings and persistence layer for `vidgrab-core`.
//!
//! Stores the download history, active queue (for crash/restart recovery of `.part` files),
//! and channel/playlist subscriptions. Also supports full JSON export and import.

use crate::error::{CoreError, CoreResult};
use chrono::Utc;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;
use uuid::Uuid;
use vidgrab_shared::{DownloadItem, DownloadStatus, ExportBundle, Subscription};

#[repr(C)]
struct sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct sqlite3_stmt {
    _private: [u8; 0],
}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_TRANSIENT: isize = -1;

#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        z_sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut sqlite3_stmt,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        index: c_int,
        value: *const c_char,
        n: c_int,
        destructor: *const c_void,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i_col: c_int) -> *const c_char;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
}

struct SqliteConnection {
    raw: *mut sqlite3,
}

// SAFETY: Access to `SqliteConnection` inside `Database` is serialized through a `std::sync::Mutex`.
unsafe impl Send for SqliteConnection {}

impl Drop for SqliteConnection {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `self.raw` was initialized by `sqlite3_open` and is closed once on drop.
            unsafe {
                sqlite3_close(self.raw);
            }
        }
    }
}

/// Thread-safe SQLite database handle for downloads and subscriptions.
pub struct Database {
    conn: Mutex<SqliteConnection>,
}

impl Database {
    /// Opens or creates the SQLite database at `db_path` and runs schema migrations.
    pub fn open(db_path: &Path) -> CoreResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path_str = db_path.to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())
            .map_err(|e| CoreError::Database(format!("Invalid database path: {e}")))?;

        let mut raw: *mut sqlite3 = ptr::null_mut();
        // SAFETY: `c_path` is a valid null-terminated C string and `raw` receives the handle.
        let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut raw) };
        if rc != SQLITE_OK || raw.is_null() {
            return Err(CoreError::Database(format!(
                "sqlite3_open failed with code {rc}"
            )));
        }

        let db = Self {
            conn: Mutex::new(SqliteConnection { raw }),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> CoreResult<()> {
        let sql = "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS downloads (
                id TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS subscriptions (
                id TEXT PRIMARY KEY NOT NULL,
                url TEXT NOT NULL,
                created_at TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );
        ";
        self.exec_raw(sql)
    }

    fn exec_raw(&self, sql: &str) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::exec_raw_locked(guard.raw, sql)
    }

    fn exec_raw_locked(raw: *mut sqlite3, sql: &str) -> CoreResult<()> {
        let c_sql = CString::new(sql)
            .map_err(|e| CoreError::Database(format!("Invalid SQL string: {e}")))?;
        // SAFETY: `raw` is an open SQLite handle owned by the locked mutex guard and `c_sql` is NUL-terminated.
        let rc =
            unsafe { sqlite3_exec(raw, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
        if rc != SQLITE_OK {
            return Err(CoreError::Database(Self::last_error(raw)));
        }
        Ok(())
    }

    fn run_in_transaction<T>(
        &self,
        op: impl FnOnce(*mut sqlite3) -> CoreResult<T>,
    ) -> CoreResult<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::exec_raw_locked(guard.raw, "BEGIN IMMEDIATE;")?;
        match op(guard.raw) {
            Ok(val) => {
                if let Err(err) = Self::exec_raw_locked(guard.raw, "COMMIT;") {
                    let _ = Self::exec_raw_locked(guard.raw, "ROLLBACK;");
                    return Err(err);
                }
                Ok(val)
            }
            Err(err) => {
                let _ = Self::exec_raw_locked(guard.raw, "ROLLBACK;");
                Err(err)
            }
        }
    }

    fn last_error(raw: *mut sqlite3) -> String {
        // SAFETY: `sqlite3_errmsg` returns a valid NUL-terminated string pointer for `raw`.
        unsafe {
            let ptr = sqlite3_errmsg(raw);
            if ptr.is_null() {
                String::from("Unknown SQLite error")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    fn upsert_download_locked(raw: *mut sqlite3, item: &DownloadItem) -> CoreResult<()> {
        let id = item.request.id.to_string();
        let status = serde_json::to_string(&item.progress.status)?;
        let created_at = item.created_at.to_rfc3339();
        let payload = serde_json::to_string(item)?;

        let sql = "INSERT INTO downloads (id, status, created_at, payload_json)
                   VALUES (?1, ?2, ?3, ?4)
                   ON CONFLICT(id) DO UPDATE SET
                       status = excluded.status,
                       payload_json = excluded.payload_json;";

        Self::execute_bound_locked(raw, sql, &[&id, &status, &created_at, &payload])
    }

    /// Inserts or updates a [`DownloadItem`] in the `downloads` table.
    pub fn upsert_download(&self, item: &DownloadItem) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::upsert_download_locked(guard.raw, item)
    }

    /// Removes a download item by ID.
    pub fn delete_download(&self, id: Uuid) -> CoreResult<()> {
        let id_str = id.to_string();
        self.execute_bound("DELETE FROM downloads WHERE id = ?1;", &[&id_str])
    }

    /// Removes a batch of download items inside a single atomic SQLite transaction.
    pub fn delete_downloads_batch(&self, ids: &[Uuid]) -> CoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.run_in_transaction(|raw| {
            for id in ids {
                let id_str = id.to_string();
                Self::execute_bound_locked(
                    raw,
                    "DELETE FROM downloads WHERE id = ?1;",
                    &[&id_str],
                )?;
            }
            Ok(())
        })
    }

    /// Loads all download items ordered by `created_at` descending.
    pub fn list_downloads(&self) -> CoreResult<Vec<DownloadItem>> {
        let rows = self
            .query_single_column("SELECT payload_json FROM downloads ORDER BY created_at DESC;")?;
        let mut items = Vec::with_capacity(rows.len());
        for json in rows {
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
        self.run_in_transaction(|raw| {
            for item in &mut to_recover {
                item.progress.status = DownloadStatus::Queued;
                Self::upsert_download_locked(raw, item)?;
            }
            Ok(())
        })?;
        Ok(to_recover)
    }

    fn upsert_subscription_locked(raw: *mut sqlite3, sub: &Subscription) -> CoreResult<()> {
        let id = sub.id.to_string();
        let url = sub.url.clone();
        let created_at = sub.created_at.to_rfc3339();
        let payload = serde_json::to_string(sub)?;

        let sql = "INSERT INTO subscriptions (id, url, created_at, payload_json)
                   VALUES (?1, ?2, ?3, ?4)
                   ON CONFLICT(id) DO UPDATE SET
                       url = excluded.url,
                       payload_json = excluded.payload_json;";

        Self::execute_bound_locked(raw, sql, &[&id, &url, &created_at, &payload])
    }

    /// Inserts or updates a [`Subscription`] in the `subscriptions` table.
    pub fn upsert_subscription(&self, sub: &Subscription) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::upsert_subscription_locked(guard.raw, sub)
    }

    /// Deletes a subscription by ID.
    pub fn delete_subscription(&self, id: Uuid) -> CoreResult<()> {
        let id_str = id.to_string();
        self.execute_bound("DELETE FROM subscriptions WHERE id = ?1;", &[&id_str])
    }

    /// Lists all subscriptions ordered by `created_at` descending.
    pub fn list_subscriptions(&self) -> CoreResult<Vec<Subscription>> {
        let rows = self.query_single_column(
            "SELECT payload_json FROM subscriptions ORDER BY created_at DESC;",
        )?;
        let mut subs = Vec::with_capacity(rows.len());
        for json in rows {
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
        self.run_in_transaction(|raw| {
            for dl in &bundle.downloads {
                Self::upsert_download_locked(raw, dl)?;
            }
            for sub in &bundle.subscriptions {
                Self::upsert_subscription_locked(raw, sub)?;
            }
            Ok((bundle.downloads.len(), bundle.subscriptions.len()))
        })
    }

    fn execute_bound(&self, sql: &str, params: &[&str]) -> CoreResult<()> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        Self::execute_bound_locked(guard.raw, sql, params)
    }

    fn execute_bound_locked(raw: *mut sqlite3, sql: &str, params: &[&str]) -> CoreResult<()> {
        let c_sql = CString::new(sql)
            .map_err(|e| CoreError::Database(format!("Invalid SQL string: {e}")))?;

        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        // SAFETY: Prepares `c_sql` on locked `raw` and ensures `sqlite3_finalize` is always called.
        unsafe {
            let rc = sqlite3_prepare_v2(raw, c_sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
            if rc != SQLITE_OK || stmt.is_null() {
                return Err(CoreError::Database(Self::last_error(raw)));
            }

            for (i, param) in params.iter().enumerate() {
                if param.as_bytes().contains(&0) {
                    sqlite3_finalize(stmt);
                    return Err(CoreError::Database(
                        "Interior NUL byte is not allowed in SQL parameter".to_string(),
                    ));
                }
                let len = c_int::try_from(param.len()).map_err(|_| {
                    sqlite3_finalize(stmt);
                    CoreError::Database("SQL parameter exceeds c_int::MAX".to_string())
                })?;
                // Pass a non-null pointer with length 0 for empty strings so SQLite binds "" instead of NULL.
                let val_ptr = if param.is_empty() {
                    c"".as_ptr()
                } else {
                    param.as_ptr() as *const c_char
                };
                let b_rc = sqlite3_bind_text(
                    stmt,
                    (i + 1) as c_int,
                    val_ptr,
                    len,
                    SQLITE_TRANSIENT as *const c_void,
                );
                if b_rc != SQLITE_OK {
                    sqlite3_finalize(stmt);
                    return Err(CoreError::Database(Self::last_error(raw)));
                }
            }

            let step_rc = sqlite3_step(stmt);
            sqlite3_finalize(stmt);
            if step_rc != SQLITE_DONE && step_rc != SQLITE_ROW {
                return Err(CoreError::Database(Self::last_error(raw)));
            }
        }
        Ok(())
    }

    fn query_single_column(&self, sql: &str) -> CoreResult<Vec<String>> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| CoreError::Database("Database mutex poisoned".to_string()))?;
        let c_sql = CString::new(sql)
            .map_err(|e| CoreError::Database(format!("Invalid SQL string: {e}")))?;

        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        let mut results = Vec::new();
        // SAFETY: Steps through prepared statement rows and finalizes `stmt` before returning.
        unsafe {
            let rc = sqlite3_prepare_v2(guard.raw, c_sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
            if rc != SQLITE_OK || stmt.is_null() {
                return Err(CoreError::Database(Self::last_error(guard.raw)));
            }

            loop {
                let step_rc = sqlite3_step(stmt);
                if step_rc == SQLITE_ROW {
                    let text_ptr = sqlite3_column_text(stmt, 0);
                    if !text_ptr.is_null() {
                        let val = CStr::from_ptr(text_ptr).to_string_lossy().into_owned();
                        results.push(val);
                    }
                } else if step_rc == SQLITE_DONE {
                    break;
                } else {
                    sqlite3_finalize(stmt);
                    return Err(CoreError::Database(Self::last_error(guard.raw)));
                }
            }
            sqlite3_finalize(stmt);
        }
        Ok(results)
    }
}
