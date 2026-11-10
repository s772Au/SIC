#[path = "../src/decision.rs"]
mod decision;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/legacy.rs"]
mod legacy;
#[path = "../src/legacy_import.rs"]
mod legacy_import;
#[path = "../src/storage.rs"]
mod storage;

use legacy_import::import_stopped_text_backlog;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Executor,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;
use tempfile::tempdir;

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

async fn fixture(path: &Path, active_or_sidecar: bool, corrupt_wb: bool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    for statement in [
        "CREATE TABLE objects(id TEXT PRIMARY KEY,kind TEXT NOT NULL,data TEXT NOT NULL,digest TEXT NOT NULL)",
        "CREATE TABLE chains(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE boards(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE revocations(id TEXT PRIMARY KEY,record TEXT NOT NULL)",
        "CREATE TABLE receipts(key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result TEXT NOT NULL)",
    ] {
        pool.execute(statement).await.unwrap();
    }
    let source = json!({"format":"openai-chat","protected":[],"open_calls":[],
        "sidecar":{"messages":[{"role":"user","content":"fixture text"}]},
        "units":[{"unit_id":"u1","text":"fixture text","anchor":{"message":0,"part":null},"kind":"text"}]});
    let source_text = serde_json::to_string(&source).unwrap();
    sqlx::query("INSERT INTO objects VALUES(?,?,?,?)")
        .bind("source-1")
        .bind("source")
        .bind(&source_text)
        .bind(digest(&source_text))
        .execute(&pool)
        .await
        .unwrap();
    let wb = json!({"wb_id":"wb-1","chain_id":"chain-1","context_id":"main","level":1,"sequence":1,
        "source_id":"source-1","source_ids":["source-1"],"unit_ids":[if corrupt_wb {"not-u1"} else {"u1"}],
        "members":[],"reason":"edit-boundary","formed_at":"synthetic"});
    let wb_text = serde_json::to_string(&wb).unwrap();
    sqlx::query("INSERT INTO objects VALUES(?,?,?,?)")
        .bind("wb-1")
        .bind("wb")
        .bind(&wb_text)
        .bind(digest(&wb_text))
        .execute(&pool)
        .await
        .unwrap();
    let mut state = json!({"chain_id":"chain-1","owners":["alice"],"creators":[],"origin_checkpoint":null,
        "cbs":[],"pending":["source-1"],"groups":{"source-1":["source-1"]},"protected":[],
        "cut_seq":0,"current_cut":null,"instant":{},"instant_draft":{},"jobs":{},"run_state":"stopped","config":{},
        "hierarchy":{"levels":{"1":{"wbs":["wb-1"],"cbs":[],"pending":["source-1"],"instant":{},"pairs":{}},
            },"active":{"worker":1,"editor":1},"contexts":{},"role_views":{}}});
    if active_or_sidecar {
        state["run_state"] = json!("running");
        state["board"] = json!({"revision":1,"value":{"board":"fixture"},"read_acl":["alice"],"write_acl":["alice"]});
    }
    let state_text = serde_json::to_string(&state).unwrap();
    sqlx::query("INSERT INTO chains VALUES(?,?)")
        .bind("chain-1")
        .bind(state_text)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn imports_only_verified_stopped_text_backlog_and_retains_raw_snapshot() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("python.sqlite");
    let destination = dir.path().join("rust.sqlite");
    fixture(&source, false, false).await;
    let before = std::fs::read(&source).unwrap();
    let report = import_stopped_text_backlog(&source, &destination)
        .await
        .unwrap();
    assert_eq!(report.imported_chain_id, "chain-1");
    assert_eq!(report.imported_source_ids, vec!["source-1"]);
    assert_eq!(report.imported_source_count, 1);
    assert!(!report.source_database_modified);
    assert_eq!(before, std::fs::read(&source).unwrap());

    let rust = storage::Store::open(&destination).await.unwrap();
    let mut tx = rust.begin().await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM chains WHERE id='chain-1'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["run_state"], "stopped");
    assert_eq!(state["groups"]["source-1"], json!(["source-1"]));
    let imported_wb: String = sqlx::query_scalar("SELECT kind FROM objects WHERE id='wb-1'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(imported_wb, "wb");
    let archive: (String, String) = sqlx::query_as("SELECT data,digest FROM objects WHERE id=?")
        .bind(&report.raw_archive_object_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(digest(&archive.0), archive.1);
    let archive: Value = serde_json::from_str(&archive.0).unwrap();
    assert_eq!(archive["archive_sha256"], report.raw_archive_sha256);
    tx.commit().await.unwrap();
    rust.close().await;
}

#[tokio::test]
async fn active_agents_and_permission_sidecars_are_refused_before_destination_creation() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("active-python.sqlite");
    let destination = dir.path().join("must-not-exist.sqlite");
    fixture(&source, true, false).await;
    let before = std::fs::read(&source).unwrap();
    assert!(
        import_stopped_text_backlog(&source, &destination)
            .await
            .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(before, std::fs::read(&source).unwrap());
}

#[tokio::test]
async fn existing_destination_is_never_overwritten() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("python.sqlite");
    let destination = dir.path().join("existing.sqlite");
    fixture(&source, false, false).await;
    std::fs::write(&destination, b"preserve existing destination").unwrap();
    assert!(
        import_stopped_text_backlog(&source, &destination)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"preserve existing destination"
    );
}

#[tokio::test]
async fn inconsistent_wb_is_refused_without_creating_target() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("bad-wb.sqlite");
    let destination = dir.path().join("rust.sqlite");
    fixture(&source, false, true).await;
    assert!(
        import_stopped_text_backlog(&source, &destination)
            .await
            .is_err()
    );
    assert!(!destination.exists());
}
