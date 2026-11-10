#[path = "../src/error.rs"]
mod error;
#[path = "../src/legacy.rs"]
mod legacy;

use legacy::{inspect_python_database, require_migration_ready, snapshot_python_records};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    Executor,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;
use tempfile::tempdir;

fn digest(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

async fn python_core_fixture(path: &Path, state: &str, object_kind: &str, bad_object_hash: bool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE objects(id TEXT PRIMARY KEY,kind TEXT NOT NULL,data TEXT NOT NULL,digest TEXT NOT NULL)",
        "CREATE TABLE chains(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE boards(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE revocations(id TEXT PRIMARY KEY,record TEXT NOT NULL)",
        "CREATE TABLE receipts(key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result TEXT NOT NULL)",
        "CREATE TABLE runtime_queue(id TEXT PRIMARY KEY,chain_id TEXT,actor TEXT,event TEXT,status TEXT)",
        "CREATE TABLE host_tool_tasks(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE user_extension(id TEXT PRIMARY KEY,private_payload TEXT NOT NULL)",
        "CREATE TRIGGER objects_immutable BEFORE UPDATE ON objects BEGIN SELECT RAISE(ABORT,'immutable'); END",
    ] {
        pool.execute(sql).await.unwrap();
    }
    let object = r#"{"id":"source-1","units":[],"sidecar":{}}"#;
    let object_hash = if bad_object_hash {
        "incorrect".to_owned()
    } else {
        digest(object)
    };
    sqlx::query("INSERT INTO objects VALUES(?,?,?,?)")
        .bind("source-1")
        .bind(object_kind)
        .bind(object)
        .bind(object_hash)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chains VALUES(?,?)")
        .bind("chain-1")
        .bind(state)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_queue VALUES(?,?,?,?,?)")
        .bind("event-1")
        .bind("chain-1")
        .bind("actor")
        .bind("{}" /* synthetic only */)
        .bind("pending")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO host_tool_tasks VALUES(?,?)")
        .bind("task-1")
        .bind(r#"{"status":"running"}"#)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_extension VALUES(?,?)")
        .bind("row-1")
        .bind("sensitive fixture payload")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn compatibility_report_is_metadata_only_and_refuses_active_unknown_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("python-core.sqlite");
    let state = json!({
        "chain_id":"chain-1", "owners":["actor"], "creators":[], "origin_checkpoint":null,
        "groups":{}, "hierarchy":{"levels":{"1":{"cbs":[],"pending":[]}}},
        "jobs":{"job-1":{"status":"open"}}, "provider_inflight":{"call_id":"opaque"},
        "run_state":"running", "future_python_field":{"enabled":true}
    })
    .to_string();
    python_core_fixture(&path, &state, "source", false).await;
    let report = inspect_python_database(&path).await.unwrap();
    assert!(report.sqlite_integrity_ok);
    assert!(report.recognized_python_core_store);
    assert!(report.unknown_tables.contains(&"user_extension".to_owned()));
    assert_eq!(report.object_kind_counts["source"], 1);
    assert_eq!(report.chain_count, 1);
    assert_eq!(report.chain_run_state_counts["running"], 1);
    assert_eq!(report.active_chain_count, 1);
    assert_eq!(report.active_job_count, 1);
    assert_eq!(report.provider_inflight_chain_count, 1);
    assert_eq!(report.pending_runtime_event_count, 1);
    assert_eq!(report.active_host_task_count, 1);
    assert!(
        report
            .unknown_chain_top_level_key_counts
            .contains_key("future_python_field")
    );
    assert!(
        report
            .missing_rust_agent_required_key_counts
            .contains_key("contexts")
    );
    assert!(
        report
            .missing_rust_agent_required_key_counts
            .contains_key("role_views")
    );
    assert!(!report.migration_attempted);
    assert!(!report.migration_ready);
    assert!(report.writes_performed == false);
    assert!(require_migration_ready(&report).is_err());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("sensitive fixture payload")
    );
    assert!(
        report
            .blockers
            .iter()
            .any(|item| item.contains("no full-fidelity"))
    );
}

#[tokio::test]
async fn raw_snapshot_retains_unknown_tables_and_data_without_agent_adoption() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("python-core.sqlite");
    let state = r#"{"chain_id":"chain-1","run_state":"waiting","hierarchy":{"levels":{}}}"#;
    python_core_fixture(&path, state, "source", false).await;
    let before = std::fs::read(&path).unwrap();
    let archive = snapshot_python_records(&path).await.unwrap();
    archive.verify().unwrap();
    assert!(archive.raw_only);
    assert!(!archive.executable_import);
    assert!(!archive.original_modified);
    assert!(
        archive
            .tables
            .iter()
            .any(|table| table.name == "user_extension")
    );
    let extension = archive
        .tables
        .iter()
        .find(|table| table.name == "user_extension")
        .unwrap();
    assert_eq!(extension.rows.len(), 1);
    assert_eq!(extension.row_ids, vec![Some(1)]);
    let payload_cell = &extension.rows[0][1];
    assert_eq!(payload_cell.storage_class, "TEXT");
    assert_eq!(
        payload_cell.value["hex"],
        hex_encode("sensitive fixture payload")
    );
    assert_eq!(before, std::fs::read(&path).unwrap());
}

#[tokio::test]
async fn non_core_legacy_schema_is_archivable_but_not_misidentified_as_agent_store() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    pool.execute("CREATE TABLE sessions(id TEXT PRIMARY KEY,name TEXT NOT NULL)")
        .await
        .unwrap();
    pool.execute("INSERT INTO sessions VALUES('s','synthetic')")
        .await
        .unwrap();
    pool.close().await;
    let report = inspect_python_database(&path).await.unwrap();
    assert!(!report.recognized_python_core_store);
    assert!(report.unknown_tables.contains(&"sessions".to_owned()));
    assert!(!report.migration_ready);
    let archive = snapshot_python_records(&path).await.unwrap();
    assert!(archive.verify().is_ok());
    assert!(archive.tables.iter().any(|table| table.name == "sessions"));
}

#[tokio::test]
async fn malformed_json_digest_mismatch_and_unknown_object_kind_are_reported() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad-object.sqlite");
    let state = r#"{"chain_id":"chain-1","run_state":"stopped"}"#;
    python_core_fixture(&path, state, "future_object", true).await;
    let report = inspect_python_database(&path).await.unwrap();
    assert_eq!(report.object_raw_digest_mismatches, 1);
    assert_eq!(report.unsupported_object_kind_count, 1);
    assert!(
        report
            .blockers
            .iter()
            .any(|item| item.contains("digests differ"))
    );
    assert!(
        report
            .blockers
            .iter()
            .any(|item| item.contains("object kinds"))
    );
}

#[tokio::test]
async fn required_table_names_with_incompatible_columns_are_not_accepted() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wrong-schema.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE objects(id TEXT)",
        "CREATE TABLE chains(id TEXT,state TEXT)",
        "CREATE TABLE boards(id TEXT,state TEXT)",
        "CREATE TABLE revocations(id TEXT,record TEXT)",
        "CREATE TABLE receipts(key TEXT,fingerprint TEXT,result TEXT)",
    ] {
        pool.execute(sql).await.unwrap();
    }
    pool.close().await;
    let report = inspect_python_database(&path).await.unwrap();
    assert!(!report.recognized_python_core_store);
    assert!(
        report
            .schema_issues
            .iter()
            .any(|issue| issue.starts_with("table objects"))
    );
    assert!(!report.migration_ready);
    assert!(require_migration_ready(&report).is_err());
}

#[tokio::test]
async fn unknown_chain_run_state_is_reported_and_never_promoted_to_rust_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("future-state.sqlite");
    python_core_fixture(
        &path,
        r#"{"chain_id":"chain-1","run_state":"future_run_state"}"#,
        "source",
        false,
    )
    .await;
    let report = inspect_python_database(&path).await.unwrap();
    assert_eq!(report.chain_run_state_counts["future_run_state"], 1);
    assert!(!report.migration_ready);
    assert!(require_migration_ready(&report).is_err());
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
