//! Transactional storage for the Rust core. Legacy databases are never opened
//! for writing implicitly; import must be an explicit, separately verified step.
use crate::error::{CoreError, Result};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{
    Row, Sqlite, SqliteConnection, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    collections::HashMap,
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
pub(crate) fn native_collection_proofs(source: &Value) -> Vec<&Value> {
    match source.get("provenances").and_then(Value::as_array) {
        Some(proofs) => proofs.iter().filter(|proof| proof.is_object()).collect(),
        None => source.get("provenance").filter(|proof| proof.is_object()).into_iter().collect(),
    }
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
    "CREATE TABLE IF NOT EXISTS sensitivity(id TEXT PRIMARY KEY,record TEXT NOT NULL)",
    "CREATE TRIGGER IF NOT EXISTS immutable_sensitivity_update BEFORE UPDATE ON sensitivity BEGIN SELECT RAISE(ABORT,'immutable sensitivity'); END",
    "CREATE TRIGGER IF NOT EXISTS immutable_sensitivity_delete BEFORE DELETE ON sensitivity BEGIN SELECT RAISE(ABORT,'immutable sensitivity'); END",
    "CREATE TABLE IF NOT EXISTS receipts(key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS runtime_queue(id TEXT PRIMARY KEY,chain_id TEXT,actor TEXT,event TEXT,status TEXT)",
    "CREATE TABLE IF NOT EXISTS bridge_bindings(actor TEXT NOT NULL,session_key TEXT NOT NULL,state TEXT NOT NULL,lease TEXT,lease_until REAL NOT NULL DEFAULT 0,lease_pid INTEGER,PRIMARY KEY(actor,session_key))",
    "CREATE TABLE IF NOT EXISTS bridge_turns(actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(actor,session_key,fingerprint))",
    "CREATE TABLE IF NOT EXISTS host_tool_hosts(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS host_tool_tasks(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
];

const NATIVE_OBJECT_INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS native_versions_by_source ON objects(json_extract(data,'$.source_id'), CAST(json_extract(data,'$.version') AS INTEGER) DESC) WHERE kind='native_version'",
    "CREATE INDEX IF NOT EXISTS native_publications_by_source ON objects(json_extract(data,'$.source_id')) WHERE kind='native_publication'",
    "CREATE INDEX IF NOT EXISTS native_projections_by_source ON objects(json_extract(data,'$.source_id')) WHERE kind='native_i_projection'",
    "CREATE INDEX IF NOT EXISTS native_sources_by_core_root ON objects(json_extract(data,'$.core_source_id')) WHERE kind='native_source'",
];

impl Store {
    /// Create a new database only. Never replace or initialize an existing file.
    pub async fn create(path: &Path) -> Result<Self> {
        Self::create_with_connections(path, 1).await
    }

    /// Test/host helper: create a file-backed store with a bounded pool so
    /// independent transactions can actually overlap. The default remains 1.
    pub async fn create_with_connections(path: &Path, max_connections: u32) -> Result<Self> {
        if max_connections == 0 {
            return Err(CoreError::invalid("max connections must be positive"));
        }
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
                .max_connections(max_connections)
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
                // SQLx drops an acquired connection if cancellation interrupts
                // its async health check. For a one-connection memory database
                // that destroys the schema. With no acquisition callbacks,
                // disabling the ping removes the cancellation window between
                // taking the idle connection and handing it to the caller.
                .test_before_acquire(false)
                .connect_with(options)
                .await?,
            bridge_guard_state: bridge_guard_state(None),
        };
        store.initialize().await?;
        Ok(store)
    }

    /// Verify the Rust schema with a read-only probe before creating a service
    /// lock file or opening the database for writes. This is not an exclusive
    /// guard; service callers must acquire their OS lock before `open`.
    pub async fn verify_existing(path: &Path) -> Result<()> {
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
        Ok(())
    }

    /// Reopen only a database previously created by this Rust core. The initial
    /// probe is read-only, so an old Python database cannot acquire WAL changes.
    pub async fn open(path: &Path) -> Result<Self> {
        Self::open_with_connections(path, 1).await
    }

    /// Test/host helper: reopen an existing database with a bounded pool so
    /// independent Core instances can exercise real CAS contention.
    pub async fn open_with_connections(path: &Path, max_connections: u32) -> Result<Self> {
        if max_connections == 0 {
            return Err(CoreError::invalid("max connections must be positive"));
        }
        Self::verify_existing(path).await?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let canonical_path = path
            .canonicalize()
            .map_err(|_| CoreError::invalid("cannot resolve database path"))?;
        let store = Self {
            pool: SqlitePoolOptions::new()
                .max_connections(max_connections)
                .connect_with(options)
                .await?,
            bridge_guard_state: bridge_guard_state(Some(bridge_lock_path(&canonical_path))),
        };
        // This is a verified Rust database. Index creation changes neither
        // immutable records nor their digests and is safe to repeat on reopen.
        let mut tx = store.begin().await?;
        for statement in NATIVE_OBJECT_INDEXES {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(store)
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
        for statement in SCHEMA.iter().chain(NATIVE_OBJECT_INDEXES) {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO sic_rust_meta(key,value) VALUES('schema','sic-rust-core-v1')")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn begin(&self) -> Result<Transaction<'_, Sqlite>> {
        // Keep queueing/acquisition cancellable. Only a connection already
        // acquired from this bounded pool may enter the protected handoff.
        let connection = self.pool.acquire().await?;
        // SQLx's custom BEGIN path awaits a handle check after SQLite has
        // acknowledged BEGIN, before constructing the rollback-on-drop guard.
        // Cancelling in that gap leaves an unowned transaction on the pooled
        // connection. Finish constructing the guard in a task that is not
        // aborted when its JoinHandle is dropped. If the caller goes away,
        // the unclaimed Transaction is dropped and rolls back normally.
        let transaction = tokio::spawn(async move {
            Transaction::begin(connection, Some("BEGIN IMMEDIATE".into())).await
        })
        .await
        .map_err(|_| CoreError::storage())??;
        Ok(transaction)
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
    VerifiedObjects::default().read(conn, id, expected_kind).await
}

/// Reuse dependency checks within one read snapshot. A caller must never carry
/// this across transactions or a revocation write. JSON bodies are not cached.
#[derive(Default)]
pub(crate) struct VerifiedObjects {
    kinds: HashMap<String, String>,
    archive_board: Option<Option<String>>,
}

#[derive(Deserialize)]
struct SourceDependencies {
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    cb_dependencies: Vec<String>,
    #[serde(default)]
    native_source_dependencies: Vec<String>,
    #[serde(default)]
    native_i_board_id: Option<String>,
}

impl VerifiedObjects {
    pub(crate) fn in_archive_scope(board: Option<&str>) -> Self {
        Self { kinds: HashMap::new(), archive_board: Some(board.map(str::to_owned)) }
    }

    fn check_archive_board(&self, origin: Option<&str>) -> Result<()> {
        if let (Some(current), Some(origin)) = (&self.archive_board, origin)
            && current.as_deref() != Some(origin)
        {
            return Err(CoreError::forbidden("archived source board is no longer bound"));
        }
        Ok(())
    }

    pub(crate) async fn verify(
        &mut self,
        conn: &mut SqliteConnection,
        id: &str,
        kind: &str,
    ) -> Result<()> {
        match self.kinds.get(id) {
            Some(verified) if verified != kind => Err(CoreError::new("not_found", "object kind mismatch", 404)),
            Some(_) => Ok(()),
            None => self.walk(conn, id, Some(kind), false).await.map(|_| ()),
        }
    }

    pub(crate) async fn read(
        &mut self,
        conn: &mut SqliteConnection,
        id: &str,
        expected_kind: Option<&str>,
    ) -> Result<Value> {
        self.walk(conn, id, expected_kind, true).await
    }

    async fn walk(
        &mut self,
        conn: &mut SqliteConnection,
        id: &str,
        expected_kind: Option<&str>,
        return_root: bool,
    ) -> Result<Value> {
        let mut stack = vec![(id.to_owned(), expected_kind.map(str::to_owned))];
        let mut visited = HashMap::<String, String>::new();
        let mut root = None;
        let mut first = true;
        while let Some((current, expected)) = stack.pop() {
            let needs_value = first && return_root;
            first = false;
            let known = visited.get(&current).or_else(|| self.kinds.get(&current));
            if let Some(kind) = known {
                if expected.as_ref().is_some_and(|value| value != kind) {
                    return Err(CoreError::new("not_found", "object kind mismatch", 404));
                }
            }
            if !needs_value && known.is_some() {
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
            // Verify the entire immutable JSON digest even when only its
            // dependency references are needed. Large native sidecars stay
            // out of the Value tree when traversing archived source links.
            if kind == "source" && !needs_value {
                let dependencies: SourceDependencies = serde_json::from_str(&data)?;
                self.check_archive_board(dependencies.native_i_board_id.as_deref())?;
                for (ids, child_kind) in [
                    (dependencies.dependencies, "source"),
                    (dependencies.cb_dependencies, "cb"),
                    (dependencies.native_source_dependencies, "native_source"),
                ] {
                    stack.extend(ids.into_iter().map(|id| (id, Some(child_kind.to_owned()))));
                }
                visited.insert(current, kind);
                continue;
            }
            let value: Value = serde_json::from_str(&data)?;
            self.check_archive_board(match kind.as_str() {
                "source" => value["native_i_board_id"].as_str(),
                "native_source" => value["adopted_i"]["board_id"].as_str(),
                "cut" => value["board_id"].as_str(),
                _ => None,
            })?;
            if known.is_some() {
                root = Some(value);
                continue;
            }
            visited.insert(current, kind.clone());
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
                "native_source" => {
                    push_values("protected_cb_ids", Some("cb"))?;
                    if let Some(cut)=value["adopted_i"]["cut_id"].as_str() {stack.push((cut.to_owned(),Some("cut".into())));}
                    for provenance in native_collection_proofs(&value) {
                        for (key,kind) in [("lifecycle_object_id","harness_object"),("collection_receipt_id","harness_event")] {
                            let id = provenance[key].as_str().ok_or_else(||CoreError::invalid("native provenance dependency missing"))?;
                            stack.push((id.to_owned(),Some(kind.into())));
                        }
                    }
                    if let Some(dep) = value.get("core_source_id").and_then(Value::as_str) {
                        stack.push((dep.to_owned(), Some("source".into())));
                    }
                }
                "native_i_projection" => {
                    for (key,kind) in [("source_id","native_source"),("publication_id","native_publication"),("cut_id","cut")] {
                        if let Some(id)=value[key].as_str() {stack.push((id.to_owned(),Some(kind.into())));}
                    }
                }
                "native_version" | "native_publication" => {
                    let source = value.get("source_id").and_then(Value::as_str)
                        .ok_or_else(|| CoreError::invalid("native source dependency missing"))?;
                    stack.push((source.to_owned(), Some("native_source".into())));
                    if let Some(cb) = value.get("cb_id").and_then(Value::as_str) {
                        stack.push((cb.to_owned(), Some("cb".into())));
                    }
                }
                "source" => {
                    push_values("dependencies", Some("source"))?;
                    push_values("cb_dependencies", Some("cb"))?;
                    push_values("native_source_dependencies", Some("native_source"))?;
                }
                "cb" => push_values("source_ids", Some("source"))?,
                "wb" if self.archive_board.is_some() => push_values("source_ids", Some("source"))?,
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
            if needs_value {
                root = Some(value);
            }
        }
        self.kinds.extend(visited);
        if return_root {
            root.ok_or_else(|| CoreError::new("not_found", "object not found", 404))
        } else {
            Ok(Value::Null)
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn verified_existing_store_rebuilds_native_lookup_indexes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing-core.db");
        let store = Store::create(&path).await.unwrap();
        sqlx::query("DROP INDEX native_versions_by_source")
            .execute(&store.pool)
            .await
            .unwrap();
        store.close().await;

        let reopened = Store::open(&path).await.unwrap();
        let steps = sqlx::query("EXPLAIN QUERY PLAN SELECT id FROM objects WHERE kind='native_version' AND json_extract(data,'$.source_id')=? ORDER BY CAST(json_extract(data,'$.version') AS INTEGER) DESC LIMIT 1")
            .bind("native-source")
            .fetch_all(&reopened.pool)
            .await
            .unwrap();
        assert!(steps.iter().any(|step| step.get::<String, _>("detail").contains("native_versions_by_source")));
        reopened.close().await;
    }
}
