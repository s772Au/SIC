//! Read-only inspection and lossless logical snapshots of Python SIC SQLite
//! stores. The snapshot is archival data only; this module does not turn legacy
//! chains or objects into executable Rust Agents.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, Sqlite, Transaction, TypeInfo, ValueRef,
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
};

use crate::error::{CoreError, Result};

const REQUIRED_TABLES: &[&str] = &["objects", "chains", "boards", "revocations", "receipts"];
const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
    ("objects", &["id", "kind", "data", "digest"]),
    ("chains", &["id", "state"]),
    ("boards", &["id", "state"]),
    ("revocations", &["id", "record"]),
    ("receipts", &["key", "fingerprint", "result"]),
];
const KNOWN_TABLES: &[&str] = &[
    "objects",
    "chains",
    "boards",
    "revocations",
    "receipts",
    "runtime_queue",
    "bridge_bindings",
    "bridge_turns",
    "host_tool_hosts",
    "host_tool_tasks",
];
const RUST_AGENT_REQUIRED_FIELDS: &[&str] = &[
    "chain_id",
    "owners",
    "creators",
    "origin_checkpoint",
    "groups",
    "contexts",
    "role_views",
    "jobs",
    "protected",
    "current_cut",
    "cut_seq",
    "run_state",
    "config",
    "decision",
    "events",
];

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableReport {
    pub name: String,
    pub row_count: u64,
    pub columns: Vec<String>,
    pub recognized_python_table: bool,
}

/// Metadata only: no record bodies, prompts, credentials, or source text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompatibilityReport {
    pub schema: String,
    pub sqlite_integrity_ok: bool,
    pub recognized_python_core_store: bool,
    pub tables: Vec<TableReport>,
    pub schema_issues: Vec<String>,
    pub unknown_tables: Vec<String>,
    pub object_kind_counts: BTreeMap<String, u64>,
    pub malformed_object_json: u64,
    pub object_raw_digest_mismatches: u64,
    pub chain_count: u64,
    pub chain_run_state_counts: BTreeMap<String, u64>,
    pub chain_top_level_key_counts: BTreeMap<String, u64>,
    pub unknown_chain_top_level_key_counts: BTreeMap<String, u64>,
    pub missing_rust_agent_required_key_counts: BTreeMap<String, u64>,
    pub active_chain_count: u64,
    pub active_job_count: u64,
    pub provider_inflight_chain_count: u64,
    pub pending_runtime_event_count: u64,
    pub active_host_task_count: u64,
    pub unsupported_object_kind_count: u64,
    pub migration_attempted: bool,
    pub migration_ready: bool,
    pub blockers: Vec<String>,
    pub read_only_open: bool,
    pub writes_performed: bool,
}

/// A SQLite value encoded without lossy JSON coercion. TEXT/BLOB are raw bytes
/// in lowercase hex; REAL stores IEEE-754 bits; INTEGER stores the exact i64.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawCell {
    pub storage_class: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawTable {
    pub name: String,
    pub create_sql: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<RawCell>>,
    pub row_ids: Vec<Option<i64>>,
    pub row_sha256: Vec<String>,
}

/// Fully retained logical table data for an adapter to inspect later. This is
/// potentially sensitive and may be large. `executable_import` is always false.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyRawSnapshot {
    pub schema: String,
    pub tables: Vec<RawTable>,
    pub schema_objects: Vec<Value>,
    pub header_pragmas: BTreeMap<String, String>,
    pub archive_sha256: String,
    pub raw_only: bool,
    pub executable_import: bool,
    pub original_modified: bool,
}

impl LegacyRawSnapshot {
    /// Verify the independent archive digest before transferring it elsewhere.
    pub fn verify(&self) -> Result<()> {
        let payload = json!({
            "schema": self.schema,
            "tables": self.tables,
            "schema_objects": self.schema_objects,
            "header_pragmas": self.header_pragmas,
            "raw_only": self.raw_only,
            "executable_import": self.executable_import,
            "original_modified": self.original_modified,
        });
        let bytes = serde_json::to_vec(&payload)
            .map_err(|_| CoreError::invalid("legacy archive cannot be serialized"))?;
        if sha256(&bytes) != self.archive_sha256 {
            return Err(CoreError::new(
                "integrity_error",
                "legacy archive digest mismatch",
                500,
            ));
        }
        Ok(())
    }
}

async fn open_read_only(path: &Path) -> Result<SqlitePool> {
    if !path.is_file() {
        return Err(CoreError::invalid("legacy database does not exist"));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(10));
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

async fn schema_tables(tx: &mut Transaction<'_, Sqlite>) -> Result<Vec<(String, Option<String>)>> {
    let rows = sqlx::query("SELECT name,sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("name")?, row.try_get("sql")?)))
        .collect()
}

async fn table_columns(tx: &mut Transaction<'_, Sqlite>, table: &str) -> Result<Vec<String>> {
    let quoted = table.replace('"', "\"\"");
    let rows = sqlx::query(&format!("PRAGMA table_info(\"{quoted}\")"))
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|row| row.try_get("name").map_err(Into::into))
        .collect()
}

fn missing_rust_fields(value: &Value) -> Vec<&'static str> {
    RUST_AGENT_REQUIRED_FIELDS
        .iter()
        .copied()
        .filter(|key| value.get(*key).is_none())
        .collect()
}

/// Inspect a legacy database without logging or returning record bodies.
pub async fn inspect_python_database(path: &Path) -> Result<CompatibilityReport> {
    let pool = open_read_only(path).await?;
    let mut tx = pool.begin().await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *tx)
        .await?;
    let tables = schema_tables(&mut tx).await?;
    let mut table_reports = Vec::new();
    let mut table_names = BTreeSet::new();
    let mut columns_by_table = BTreeMap::new();
    for (name, _) in &tables {
        table_names.insert(name.clone());
        let columns = table_columns(&mut tx, name).await?;
        columns_by_table.insert(
            name.clone(),
            columns.iter().cloned().collect::<BTreeSet<_>>(),
        );
        let quoted = name.replace('"', "\"\"");
        let row_count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{quoted}\""))
            .fetch_one(&mut *tx)
            .await?;
        table_reports.push(TableReport {
            name: name.clone(),
            row_count: row_count.max(0) as u64,
            columns,
            recognized_python_table: KNOWN_TABLES.contains(&name.as_str()),
        });
    }

    let unknown_tables = table_names
        .iter()
        .filter(|name| !KNOWN_TABLES.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut schema_issues = Vec::new();
    for (table, required_columns) in REQUIRED_COLUMNS {
        if !table_names.contains(*table) {
            continue;
        }
        let available = columns_by_table.get(*table).cloned().unwrap_or_default();
        let missing = required_columns
            .iter()
            .filter(|column| !available.contains(**column))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            schema_issues.push(format!(
                "table {table} is missing required columns: {}",
                missing.join(",")
            ));
        }
    }
    let recognized_python_core_store = REQUIRED_TABLES
        .iter()
        .all(|name| table_names.contains(*name))
        && schema_issues.is_empty();
    let mut object_kind_counts = BTreeMap::new();
    let mut malformed_object_json = 0;
    let mut object_raw_digest_mismatches = 0;
    if columns_by_table.get("objects").is_some_and(|columns| {
        ["kind", "data", "digest"]
            .iter()
            .all(|column| columns.contains(*column))
    }) {
        let rows = sqlx::query("SELECT kind,data,digest FROM objects")
            .fetch_all(&mut *tx)
            .await?;
        for row in rows {
            let kind: String = row.try_get("kind")?;
            let data: String = row.try_get("data")?;
            let expected: String = row.try_get("digest")?;
            *object_kind_counts.entry(kind).or_insert(0) += 1;
            if serde_json::from_str::<Value>(&data).is_err() {
                malformed_object_json += 1;
            }
            if sha256(data.as_bytes()) != expected {
                object_raw_digest_mismatches += 1;
            }
        }
    }

    let mut chain_count = 0;
    let mut chain_run_state_counts = BTreeMap::new();
    let mut chain_top_level_key_counts = BTreeMap::new();
    let mut unknown_chain_top_level_key_counts = BTreeMap::new();
    let mut missing_rust_agent_required_key_counts = BTreeMap::new();
    let mut active_chain_count = 0;
    let mut active_job_count = 0;
    let mut provider_inflight_chain_count = 0;
    if columns_by_table
        .get("chains")
        .is_some_and(|columns| columns.contains("state"))
    {
        let rows = sqlx::query("SELECT state FROM chains")
            .fetch_all(&mut *tx)
            .await?;
        chain_count = rows.len() as u64;
        for row in rows {
            let raw: String = row.try_get("state")?;
            let value: Value = match serde_json::from_str(&raw) {
                Ok(value) => value,
                Err(_) => {
                    *chain_run_state_counts
                        .entry("<invalid-json>".to_owned())
                        .or_insert(0) += 1;
                    continue;
                }
            };
            if let Some(object) = value.as_object() {
                for key in object.keys() {
                    *chain_top_level_key_counts.entry(key.clone()).or_insert(0) += 1;
                }
                for key in object.keys() {
                    if !RUST_AGENT_REQUIRED_FIELDS.contains(&key.as_str()) {
                        *unknown_chain_top_level_key_counts
                            .entry(key.clone())
                            .or_insert(0) += 1;
                    }
                }
                for key in missing_rust_fields(&value) {
                    *missing_rust_agent_required_key_counts
                        .entry(key.to_owned())
                        .or_insert(0) += 1;
                }
            } else {
                *chain_run_state_counts
                    .entry("<non-object>".to_owned())
                    .or_insert(0) += 1;
                continue;
            }
            let run_state = value
                .get("run_state")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
                .to_owned();
            *chain_run_state_counts.entry(run_state.clone()).or_insert(0) += 1;
            if run_state == "running" || run_state == "waiting" {
                active_chain_count += 1;
            }
            if value
                .get("provider_inflight")
                .is_some_and(|item| !item.is_null())
            {
                provider_inflight_chain_count += 1;
            }
            if let Some(jobs) = value.get("jobs").and_then(Value::as_object) {
                active_job_count += jobs
                    .values()
                    .filter(|job| {
                        !["published", "superseded", "abandoned", "failed"]
                            .contains(&job.get("status").and_then(Value::as_str).unwrap_or(""))
                    })
                    .count() as u64;
            }
        }
    }
    let pending_runtime_event_count = if columns_by_table
        .get("runtime_queue")
        .is_some_and(|columns| columns.contains("status"))
    {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM runtime_queue WHERE status IN ('pending','running')",
        )
        .fetch_one(&mut *tx)
        .await?
        .max(0) as u64
    } else {
        0
    };
    let active_host_task_count = if columns_by_table
        .get("host_tool_tasks")
        .is_some_and(|columns| columns.contains("state"))
    {
        let rows = sqlx::query("SELECT state FROM host_tool_tasks")
            .fetch_all(&mut *tx)
            .await?;
        rows.into_iter()
            .filter_map(|row| row.try_get::<String, _>("state").ok())
            .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|state| {
                ["pending", "running", "waiting"]
                    .contains(&state.get("status").and_then(Value::as_str).unwrap_or(""))
            })
            .count() as u64
    } else {
        0
    };

    let known_object_kinds: BTreeSet<&str> = [
        "source",
        "wb",
        "cb",
        "cut",
        "checkpoint",
        "socket",
        "signal",
        "message",
        "editor_response",
        "candidate_revision",
        "decision_revision",
        "tool_result",
        "tool_return",
        "provider_request",
        "provider_response",
        "usage",
        "resource",
        "call_failure",
        "invocation_capability",
        "shared_prefix_plan",
        "editor_archive_reads",
        "editor_canonical_response",
        "ack",
        "index",
        "runtime_error",
        "runtime_recovery",
        "collaboration_retry",
        "collaboration_failure",
        "communication_receipt",
        "bridge_recovery",
    ]
    .into_iter()
    .collect();
    let unsupported_object_kind_count = object_kind_counts
        .iter()
        .filter(|(kind, _)| !known_object_kinds.contains(kind.as_str()))
        .map(|(_, count)| count)
        .sum();

    let mut blockers = Vec::new();
    if !recognized_python_core_store {
        blockers.push("required Python Core tables are missing".into());
    }
    if !schema_issues.is_empty() {
        blockers
            .push("one or more recognized tables do not match the required Python schema".into());
    }
    if !unknown_tables.is_empty() {
        blockers.push("unknown tables are retained but have no Rust converter".into());
    }
    if malformed_object_json > 0 {
        blockers.push("one or more immutable object rows are not valid JSON".into());
    }
    if object_raw_digest_mismatches > 0 {
        blockers.push(
            "one or more object raw-byte digests differ; Python canonical verification is required"
                .into(),
        );
    }
    if unsupported_object_kind_count > 0 {
        blockers.push("one or more object kinds have no declared archive adapter".into());
    }
    if active_chain_count > 0 {
        blockers.push(
            "active Python chains cannot be represented as executable Rust Agents yet".into(),
        );
    }
    if active_job_count > 0
        || provider_inflight_chain_count > 0
        || pending_runtime_event_count > 0
        || active_host_task_count > 0
    {
        blockers.push("in-flight jobs, provider calls, runtime events, or host tasks require explicit recovery semantics".into());
    }
    // Even an idle, schema-recognized DB is never declared executable until a
    // complete versioned Python-to-Rust converter and equivalence verifier exist.
    blockers.push(
        "no full-fidelity Python-to-Rust state converter is implemented; migration is refused"
            .into(),
    );
    let report = CompatibilityReport {
        schema: "sic-python-compatibility-report-v1".into(),
        sqlite_integrity_ok: integrity == "ok",
        recognized_python_core_store,
        tables: table_reports,
        schema_issues,
        unknown_tables,
        object_kind_counts,
        malformed_object_json,
        object_raw_digest_mismatches,
        chain_count,
        chain_run_state_counts,
        chain_top_level_key_counts,
        unknown_chain_top_level_key_counts,
        missing_rust_agent_required_key_counts,
        active_chain_count,
        active_job_count,
        provider_inflight_chain_count,
        pending_runtime_event_count,
        active_host_task_count,
        unsupported_object_kind_count,
        migration_attempted: false,
        migration_ready: false,
        blockers,
        read_only_open: true,
        writes_performed: false,
    };
    tx.commit().await?;
    pool.close().await;
    Ok(report)
}

/// Gate used by a future importer. This version intentionally has no executable
/// converter, so it rejects every database and leaves source files untouched.
pub fn require_migration_ready(report: &CompatibilityReport) -> Result<()> {
    if report.migration_ready {
        return Ok(());
    }
    Err(CoreError::conflict(
        "legacy migration refused: no verified full-fidelity Python-to-Rust converter is available",
    ))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn raw_cell(row: &sqlx::sqlite::SqliteRow, index: usize) -> Result<RawCell> {
    let raw = row.try_get_raw(index)?;
    if raw.is_null() {
        return Ok(RawCell {
            storage_class: "NULL".into(),
            value: Value::Null,
        });
    }
    let kind = raw.type_info().name().to_ascii_uppercase();
    let value = match kind.as_str() {
        "INTEGER" => json!(row.try_get::<i64, _>(index)?),
        "REAL" => {
            json!({"ieee754_bits": format!("{:016x}", row.try_get::<f64, _>(index)?.to_bits())})
        }
        "TEXT" => {
            let text: String = row.try_get(index)?;
            json!({"hex": hex(text.as_bytes())})
        }
        "BLOB" => {
            let bytes: Vec<u8> = row.try_get(index)?;
            json!({"hex": hex(&bytes)})
        }
        other => {
            return Err(CoreError::new(
                "unsupported_sqlite_value",
                format!("unsupported SQLite storage class {other}"),
                422,
            ));
        }
    };
    Ok(RawCell {
        storage_class: kind,
        value,
    })
}

/// Capture every table row and schema object as a typed, digest-verified raw
/// archive. The source is opened read-only; no Agent is materialized.
pub async fn snapshot_python_records(path: &Path) -> Result<LegacyRawSnapshot> {
    let pool = open_read_only(path).await?;
    let mut tx = pool.begin().await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *tx)
        .await?;
    if integrity != "ok" {
        return Err(CoreError::new(
            "integrity_error",
            "legacy source SQLite integrity check failed",
            500,
        ));
    }
    let schema_rows =
        sqlx::query("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name")
            .fetch_all(&mut *tx)
            .await?;
    let mut schema_objects = Vec::with_capacity(schema_rows.len());
    for row in schema_rows {
        schema_objects.push(json!({
            "type": row.try_get::<String, _>("type")?,
            "name": row.try_get::<String, _>("name")?,
            "table": row.try_get::<String, _>("tbl_name")?,
            "sql": row.try_get::<Option<String>, _>("sql")?,
        }));
    }
    let mut header_pragmas = BTreeMap::new();
    for pragma in [
        "application_id",
        "user_version",
        "schema_version",
        "auto_vacuum",
    ] {
        let result: i64 = sqlx::query_scalar(&format!("PRAGMA {pragma}"))
            .fetch_one(&mut *tx)
            .await?;
        header_pragmas.insert(pragma.to_owned(), result.to_string());
    }
    let encoding: String = sqlx::query_scalar("PRAGMA encoding")
        .fetch_one(&mut *tx)
        .await?;
    header_pragmas.insert("encoding".into(), encoding);
    let table_rows =
        sqlx::query("SELECT name,sql FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(&mut *tx)
            .await?;
    let tables = table_rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("name")?,
                row.try_get::<Option<String>, _>("sql")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut raw_tables = Vec::with_capacity(tables.len());
    for (name, create_sql) in tables {
        let columns = table_columns(&mut tx, &name).await?;
        let quoted = name.replace('"', "\"\"");
        let without_rowid = create_sql
            .as_deref()
            .is_some_and(|sql| sql.to_ascii_uppercase().contains("WITHOUT ROWID"));
        let row_id_name = if without_rowid {
            None
        } else {
            ["_rowid_", "rowid", "oid"]
                .into_iter()
                .find(|candidate| {
                    !columns
                        .iter()
                        .any(|column| column.eq_ignore_ascii_case(candidate))
                })
                .map(str::to_owned)
        };
        if !without_rowid && row_id_name.is_none() {
            return Err(CoreError::new(
                "unsupported_legacy_schema",
                "cannot preserve rowid because all SQLite rowid aliases are shadowed",
                422,
            ));
        }
        let select_sql = if let Some(row_id_name) = &row_id_name {
            format!("SELECT {row_id_name},* FROM \"{quoted}\" ORDER BY {row_id_name}")
        } else {
            format!("SELECT * FROM \"{quoted}\"")
        };
        let mut source_rows = sqlx::query(&select_sql).fetch(&mut *tx);
        let mut rows_with_keys = Vec::new();
        while let Some(row) = source_rows.try_next().await? {
            let row_id = if row_id_name.is_some() {
                Some(row.try_get::<i64, _>(0)?)
            } else {
                None
            };
            let offset = usize::from(row_id_name.is_some());
            let mut values = Vec::with_capacity(row.columns().len() - offset);
            for index in offset..row.columns().len() {
                values.push(raw_cell(&row, index)?);
            }
            let encoded = serde_json::to_vec(&(row_id, &values))
                .map_err(|_| CoreError::invalid("legacy row cannot be encoded"))?;
            rows_with_keys.push((encoded, values, row_id));
        }
        rows_with_keys.sort_by(|left, right| left.0.cmp(&right.0));
        let mut row_values = Vec::with_capacity(rows_with_keys.len());
        let mut row_ids = Vec::with_capacity(rows_with_keys.len());
        let mut row_hashes = Vec::with_capacity(rows_with_keys.len());
        for (encoded, values, row_id) in rows_with_keys {
            row_hashes.push(sha256(&encoded));
            row_values.push(values);
            row_ids.push(row_id);
        }
        raw_tables.push(RawTable {
            name,
            create_sql,
            columns,
            rows: row_values,
            row_ids,
            row_sha256: row_hashes,
        });
    }
    tx.commit().await?;
    pool.close().await;
    let mut snapshot = LegacyRawSnapshot {
        schema: "sic-python-raw-record-snapshot-v1".into(),
        tables: raw_tables,
        schema_objects,
        header_pragmas,
        archive_sha256: String::new(),
        raw_only: true,
        executable_import: false,
        original_modified: false,
    };
    let payload = json!({
        "schema": snapshot.schema,
        "tables": snapshot.tables,
        "schema_objects": snapshot.schema_objects,
        "header_pragmas": snapshot.header_pragmas,
        "raw_only": snapshot.raw_only,
        "executable_import": snapshot.executable_import,
        "original_modified": snapshot.original_modified,
    });
    snapshot.archive_sha256 = sha256(
        &serde_json::to_vec(&payload)
            .map_err(|_| CoreError::invalid("legacy archive cannot be serialized"))?,
    );
    Ok(snapshot)
}
