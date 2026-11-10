//! Transactional storage for the Rust core. Legacy databases are never opened
//! for writing implicitly; import must be an explicit, separately verified step.
use crate::error::{CoreError, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, Sqlite, SqliteConnection, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

pub fn uid(kind: &str) -> String {
    format!("{}_{}", kind, Uuid::new_v4().simple())
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn json_text(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|_| CoreError::invalid("value is not valid JSON"))
}
pub fn digest(value: &Value) -> Result<String> {
    Ok(sha256(json_text(value)?.as_bytes()))
}

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
    bridge_guard_state: Arc<BridgeGuardState>,
}

struct BridgeGuardState {
    active: AtomicBool,
    lock_path: Option<PathBuf>,
}

/// Owns the exclusive DSH bridge lifetime for one Store. Cloned Stores share
/// the in-memory flag, while file-backed Stores also take an OS lock so
/// independently opened Stores and other processes cannot race recovery.
pub struct StoreBridgeGuard {
    state: Arc<BridgeGuardState>,
    lock_file: Option<File>,
}

impl Drop for StoreBridgeGuard {
    fn drop(&mut self) {
        drop(self.lock_file.take());
        self.state.active.store(false, Ordering::Release);
    }
}

fn bridge_lock_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".bridge.lock");
    PathBuf::from(value)
}

fn bridge_guard_state(path: Option<PathBuf>) -> Arc<BridgeGuardState> {
    Arc::new(BridgeGuardState {
        active: AtomicBool::new(false),
        lock_path: path,
    })
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sic_rust_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS objects(id TEXT PRIMARY KEY,kind TEXT NOT NULL,data TEXT NOT NULL,digest TEXT NOT NULL)",
    "CREATE TRIGGER IF NOT EXISTS immutable_objects_update BEFORE UPDATE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END",
    "CREATE TRIGGER IF NOT EXISTS immutable_objects_delete BEFORE DELETE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END",
    "CREATE TABLE IF NOT EXISTS boards(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS chains(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS revocations(id TEXT PRIMARY KEY,record TEXT NOT NULL)",
    "CREATE TRIGGER IF NOT EXISTS immutable_revocations_update BEFORE UPDATE ON revocations BEGIN SELECT RAISE(ABORT,'immutable revocation'); END",
    "CREATE TRIGGER IF NOT EXISTS immutable_revocations_delete BEFORE DELETE ON revocations BEGIN SELECT RAISE(ABORT,'immutable revocation'); END",
    "CREATE TABLE IF NOT EXISTS receipts(key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS runtime_queue(id TEXT PRIMARY KEY,chain_id TEXT,actor TEXT,event TEXT,status TEXT)",
    "CREATE TABLE IF NOT EXISTS bridge_bindings(actor TEXT NOT NULL,session_key TEXT NOT NULL,state TEXT NOT NULL,lease TEXT,lease_until REAL NOT NULL DEFAULT 0,lease_pid INTEGER,PRIMARY KEY(actor,session_key))",
    "CREATE TABLE IF NOT EXISTS bridge_turns(actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(actor,session_key,fingerprint))",
    "CREATE TABLE IF NOT EXISTS host_tool_hosts(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS host_tool_tasks(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
];

impl Store {
    /// Create a new database only. Never replace or initialize an existing file.
    pub async fn create(path: &Path) -> Result<Self> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| CoreError::conflict("destination already exists or cannot be created"))?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let store = Self {
            pool: SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await?,
            bridge_guard_state: bridge_guard_state(Some(bridge_lock_path(
                &std::fs::canonicalize(path)
                    .map_err(|_| CoreError::invalid("cannot resolve database path"))?,
            ))),
        };
        store.initialize().await?;
        Ok(store)
    }

    /// A memory-only database is for isolation tests, not persisted user data.
    pub async fn memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let store = Self {
            pool: SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await?,
            bridge_guard_state: bridge_guard_state(None),
        };
        store.initialize().await?;
        Ok(store)
    }

    /// Reopen only a database previously created by this Rust core. The initial
    /// probe is read-only, so an old Python database cannot acquire WAL changes.
    pub async fn open(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(CoreError::invalid("Rust database does not exist"));
        }
        let ro = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .read_only(true)
                    .create_if_missing(false)
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await?;
        let marker: Option<String> =
            sqlx::query_scalar("SELECT value FROM sic_rust_meta WHERE key='schema'")
                .fetch_optional(&ro)
                .await
                .map_err(|_| {
                    CoreError::new(
                        "migration_required",
                        "database is not a verified Rust core store",
                        409,
                    )
                })?;
        ro.close().await;
        if marker.as_deref() != Some("sic-rust-core-v1") {
            return Err(CoreError::new(
                "migration_required",
                "database is not a verified Rust core store",
                409,
            ));
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let canonical_path = path
            .canonicalize()
            .map_err(|_| CoreError::invalid("cannot resolve database path"))?;
        Ok(Self {
            pool: SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await?,
            bridge_guard_state: bridge_guard_state(Some(bridge_lock_path(&canonical_path))),
        })
    }

    /// Acquire the single DSH bridge owner for this Store. Construction of a
    /// second bridge must fail before it can classify running turns as stale.
    pub fn acquire_dsh_bridge_guard(&self) -> Result<StoreBridgeGuard> {
        self.bridge_guard_state
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CoreError::conflict("a DSH bridge already owns this Store"))?;

        let lock_file = match self.bridge_guard_state.lock_path.as_ref() {
            Some(path) => {
                let result = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .and_then(|file| {
                        file.try_lock()?;
                        Ok(file)
                    });
                match result {
                    Ok(file) => Some(file),
                    Err(_) => {
                        self.bridge_guard_state
                            .active
                            .store(false, Ordering::Release);
                        return Err(CoreError::conflict(
                            "another process owns this database's DSH bridge",
                        ));
                    }
                }
            }
            None => None,
        };

        Ok(StoreBridgeGuard {
            state: self.bridge_guard_state.clone(),
            lock_file,
        })
    }

    async fn initialize(&self) -> Result<()> {
        let mut tx = self.begin().await?;
        for statement in SCHEMA {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO sic_rust_meta(key,value) VALUES('schema','sic-rust-core-v1')")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn begin(&self) -> Result<Transaction<'_, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

pub async fn put_object(
    conn: &mut SqliteConnection,
    kind: &str,
    id: &str,
    value: &Value,
) -> Result<()> {
    if kind.is_empty() || id.is_empty() {
        return Err(CoreError::invalid("object kind and id required"));
    }
    let data = json_text(value)?;
    let digest = sha256(data.as_bytes());
    sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
        .bind(id)
        .bind(kind)
        .bind(data)
        .bind(digest)
        .execute(conn)
        .await?;
    Ok(())
}

/// Read one object and all dependency revocations in the same transaction.
/// Stored JSON bytes, not a lossy float reserialization, are authenticated.
pub async fn get_object(
    conn: &mut SqliteConnection,
    id: &str,
    expected_kind: Option<&str>,
) -> Result<Value> {
    let mut stack = vec![(id.to_owned(), expected_kind.map(str::to_owned))];
    let mut visited = HashSet::new();
    let mut root = None;
    while let Some((current, expected)) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let revoked: Option<i64> = sqlx::query_scalar("SELECT 1 FROM revocations WHERE id=?")
            .bind(&current)
            .fetch_optional(&mut *conn)
            .await?;
        if revoked.is_some() {
            return Err(CoreError::forbidden("object or dependency access revoked"));
        }
        let row = sqlx::query("SELECT kind,data,digest FROM objects WHERE id=?")
            .bind(&current)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| CoreError::new("not_found", "object not found", 404))?;
        let kind: String = row.try_get("kind")?;
        let data: String = row.try_get("data")?;
        let checksum: String = row.try_get("digest")?;
        if expected.as_ref().is_some_and(|value| value != &kind) {
            return Err(CoreError::new("not_found", "object kind mismatch", 404));
        }
        if sha256(data.as_bytes()) != checksum {
            return Err(CoreError::new(
                "integrity_error",
                "stored object digest mismatch",
                500,
            ));
        }
        let value: Value = serde_json::from_str(&data)?;
        if root.is_none() {
            root = Some(value.clone());
        }
        let mut push_values = |key: &str, dep_kind: Option<&str>| -> Result<()> {
            if let Some(values) = value.get(key) {
                let values = values
                    .as_array()
                    .ok_or_else(|| CoreError::invalid("invalid object dependency list"))?;
                for value in values {
                    let dep = value
                        .as_str()
                        .ok_or_else(|| CoreError::invalid("invalid object dependency identity"))?;
                    stack.push((dep.to_owned(), dep_kind.map(str::to_owned)));
                }
            }
            Ok(())
        };
        match kind.as_str() {
            "source" => push_values("dependencies", Some("source"))?,
            "cb" => push_values("source_ids", Some("source"))?,
            "checkpoint" => {
                let cut = value
                    .get("cut_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CoreError::invalid("checkpoint cut missing"))?;
                stack.push((cut.to_owned(), Some("cut".into())));
            }
            "cut" => {
                if let Some(groups) = value.get("groups") {
                    let groups = groups
                        .as_object()
                        .ok_or_else(|| CoreError::invalid("invalid cut groups"))?;
                    for values in groups.values() {
                        for source in values
                            .as_array()
                            .ok_or_else(|| CoreError::invalid("invalid cut sources"))?
                        {
                            let source = source
                                .as_str()
                                .ok_or_else(|| CoreError::invalid("invalid cut source identity"))?;
                            stack.push((source.to_owned(), Some("source".into())));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    root.ok_or_else(|| CoreError::new("not_found", "object not found", 404))
}

pub async fn get_chain(conn: &mut SqliteConnection, id: &str, actor: &str) -> Result<Value> {
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    let state: Value = serde_json::from_str(
        &state.ok_or_else(|| CoreError::new("not_found", "chain not found", 404))?,
    )?;
    if !state
        .get("owners")
        .and_then(Value::as_array)
        .is_some_and(|owners| owners.iter().any(|owner| owner.as_str() == Some(actor)))
    {
        return Err(CoreError::forbidden("chain access denied"));
    }
    Ok(state)
}

pub async fn save_chain(conn: &mut SqliteConnection, value: &Value) -> Result<()> {
    let id = value
        .get("chain_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::invalid("chain id missing"))?;
    sqlx::query("INSERT INTO chains(id,state) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state")
        .bind(id).bind(json_text(value)?).execute(conn).await?;
    Ok(())
}
