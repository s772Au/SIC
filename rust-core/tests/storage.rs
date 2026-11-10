use serde_json::json;
use sic_core_rust::{
    archive::snapshot_python_database,
    storage::{Store, get_object, put_object, sha256},
};
use sqlx::{
    Executor,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tempfile::tempdir;

async fn python_fixture(path: &std::path::Path, bad_digest: bool) {
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
        "CREATE TRIGGER immutable_objects_update BEFORE UPDATE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END",
        "CREATE TRIGGER immutable_objects_delete BEFORE DELETE ON objects BEGIN SELECT RAISE(ABORT,'immutable object'); END",
    ] {
        pool.execute(sql).await.unwrap();
    }
    let data = r#"{"text":"中文🙂"}"#;
    let hash = if bad_digest {
        "incorrect".to_owned()
    } else {
        sha256(data.as_bytes())
    };
    sqlx::query("INSERT INTO objects VALUES(?,?,?,?)")
        .bind("source_fixture")
        .bind("source")
        .bind(data)
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_queue VALUES(?,?,?,?,?)")
        .bind("q")
        .bind("chain")
        .bind("actor")
        .bind("{}")
        .bind("pending")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn isolated_store_reopens_without_mutating_immutable_objects() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rust.db");
    let store = Store::create(&path).await.unwrap();
    let mut tx = store.begin().await.unwrap();
    put_object(&mut tx, "source", "source_1", &json!({"text":"中文🙂"}))
        .await
        .unwrap();
    assert_eq!(
        get_object(&mut tx, "source_1", Some("source"))
            .await
            .unwrap()["text"],
        "中文🙂"
    );
    assert!(
        sqlx::query("UPDATE objects SET data='{}' WHERE id='source_1'")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    put_object(&mut tx, "source", "source_2", &json!({"text":"retained"}))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    store.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let mut tx = reopened.begin().await.unwrap();
    assert_eq!(
        get_object(&mut tx, "source_2", Some("source"))
            .await
            .unwrap()["text"],
        "retained"
    );
    tx.rollback().await.unwrap();
    reopened.close().await;
    assert!(Store::create(&path).await.is_err());
}

#[tokio::test]
async fn revoked_source_blocks_derived_cb_without_rewriting_it() {
    let store = Store::memory().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    put_object(&mut tx, "source", "s", &json!({"text":"private"}))
        .await
        .unwrap();
    put_object(
        &mut tx,
        "cb",
        "c",
        &json!({"source_ids":["s"],"body":"derived"}),
    )
    .await
    .unwrap();
    assert!(get_object(&mut tx, "c", Some("cb")).await.is_ok());
    sqlx::query("INSERT INTO revocations VALUES(?,?)")
        .bind("s")
        .bind("{}")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        get_object(&mut tx, "c", Some("cb")).await.unwrap_err().code,
        "forbidden"
    );
    assert!(
        sqlx::query("DELETE FROM revocations WHERE id='s'")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn legacy_snapshot_preserves_all_tables_and_never_adopts_implicitly() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("python.db");
    let dest = dir.path().join("copy.db");
    python_fixture(&source, false).await;
    let before = std::fs::read(&source).unwrap();
    assert_eq!(
        Store::open(&source).await.err().unwrap().code,
        "migration_required"
    );
    let report = snapshot_python_database(&source, &dest).await.unwrap();
    assert!(report.sqlite_integrity_ok);
    assert_eq!(report.objects_verified, 1);
    assert_eq!(report.table_rows["runtime_queue"], 1);
    assert!(!report.original_modified);
    assert!(!report.ready_for_rust_execution);
    assert_eq!(before, std::fs::read(&source).unwrap());
    assert_eq!(
        Store::open(&dest).await.err().unwrap().code,
        "migration_required"
    );
    assert!(snapshot_python_database(&source, &dest).await.is_err());
}

#[tokio::test]
async fn corrupt_snapshot_fails_without_deleting_original_or_destination() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("bad.db");
    let dest = dir.path().join("copy.db");
    python_fixture(&source, true).await;
    let before = std::fs::read(&source).unwrap();
    assert_eq!(
        snapshot_python_database(&source, &dest)
            .await
            .unwrap_err()
            .code,
        "integrity_error"
    );
    assert_eq!(before, std::fs::read(&source).unwrap());
    assert!(dest.is_file());
}
