//! Create a coherent, separate copy of a Python Core SQLite database. This is
//! deliberately not a claim that all legacy states can already run in Rust.
use crate::{
    error::{CoreError, Result},
    storage::sha256,
};
use futures_util::TryStreamExt;
use serde::Serialize;
use sqlx::{
    Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{collections::BTreeMap, fs::OpenOptions, path::Path, time::Duration};

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotReport {
    pub sqlite_integrity_ok: bool,
    pub objects_verified: u64,
    pub table_rows: BTreeMap<String, u64>,
    pub source_changed_during_snapshot: bool,
    pub original_modified: bool,
    pub ready_for_rust_execution: bool,
}

/// SQLite `VACUUM INTO` takes a consistent view, including committed WAL data.
/// The source connection is read-only and the destination must not exist.
/// Validation never deletes or silently changes either file on failure.
pub async fn snapshot_python_database(source: &Path, destination: &Path) -> Result<SnapshotReport> {
    if !source.is_file() {
        return Err(CoreError::invalid("source database does not exist"));
    }
    let destination_text = destination
        .to_str()
        .ok_or_else(|| CoreError::invalid("destination path is not valid Unicode"))?;
    let source_options = SqliteConnectOptions::new()
        .filename(source)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(10));
    let source_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(source_options)
        .await?;
    let mut conn = source_pool.acquire().await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *conn)
    .await?;
    for required in ["objects", "chains", "boards", "revocations", "receipts"] {
        if !tables.iter().any(|name| name == required) {
            return Err(CoreError::invalid(
                "source is not a recognized Python Core database",
            ));
        }
    }
    let before: i64 = sqlx::query_scalar("PRAGMA data_version")
        .fetch_one(&mut *conn)
        .await?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| CoreError::conflict("destination already exists or cannot be created"))?;
    sqlx::query("VACUUM INTO ?")
        .bind(destination_text)
        .execute(&mut *conn)
        .await?;
    let after: i64 = sqlx::query_scalar("PRAGMA data_version")
        .fetch_one(&mut *conn)
        .await?;
    drop(conn);
    source_pool.close().await;

    let target_options = SqliteConnectOptions::new()
        .filename(destination)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(10));
    let target = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(target_options)
        .await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&target)
        .await?;
    if integrity != "ok" {
        return Err(CoreError::new(
            "integrity_error",
            "snapshot failed SQLite integrity check",
            500,
        ));
    }
    let names: Vec<String>=sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(&target).await?;
    let mut counts = BTreeMap::new();
    for name in names {
        // sqlite_master supplies the identifier; quoting prevents SQL injection.
        let quoted = name.replace('"', "\"\"");
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{quoted}\""))
            .fetch_one(&target)
            .await?;
        counts.insert(name, count as u64);
    }
    let mut verified = 0;
    {
        let mut rows = sqlx::query("SELECT data,digest FROM objects").fetch(&target);
        while let Some(row) = rows.try_next().await? {
            let data: String = row.try_get("data")?;
            let expected: String = row.try_get("digest")?;
            if sha256(data.as_bytes()) != expected
                || serde_json::from_str::<serde_json::Value>(&data).is_err()
            {
                return Err(CoreError::new(
                    "integrity_error",
                    "snapshot contains an invalid immutable object",
                    500,
                ));
            }
            verified += 1;
        }
    }
    target.close().await;
    Ok(SnapshotReport {
        sqlite_integrity_ok: true,
        objects_verified: verified,
        table_rows: counts,
        source_changed_during_snapshot: before != after,
        original_modified: false,
        ready_for_rust_execution: false,
    })
}
