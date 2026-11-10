use serde_json::{Value, json};
use sic_core_rust::{
    legacy_config::{read_target_guard, restore_legacy_config},
    legacy_upgrade::upgrade_active_python_agent,
    storage::Store,
};
use sqlx::{
    Executor, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{path::Path, process::Command};
use tempfile::tempdir;

async fn python_config_fixture(path: &Path, invalid_feedback: bool) {
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
        "CREATE TABLE runtime_queue(id TEXT PRIMARY KEY,chain_id TEXT,actor TEXT,event TEXT,status TEXT)",
        "CREATE TABLE bridge_bindings(actor TEXT NOT NULL,session_key TEXT NOT NULL,state TEXT NOT NULL,lease TEXT,lease_until REAL NOT NULL DEFAULT 0,lease_pid INTEGER,PRIMARY KEY(actor,session_key))",
        "CREATE TABLE bridge_turns(actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(actor,session_key,fingerprint))",
        "CREATE TABLE host_tool_hosts(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
        "CREATE TABLE host_tool_tasks(id TEXT PRIMARY KEY,state TEXT NOT NULL)",
    ] {
        pool.execute(statement).await.unwrap();
    }
    let feedback = if invalid_feedback {
        json!({"max_corrections":9,"feedback_chars":512})
    } else {
        json!({"max_corrections":1,"feedback_chars":1024})
    };
    let state = json!({
        "chain_id":"synthetic-python-agent",
        "owners":["synthetic-owner"],
        "creators":[],
        "run_state":"running",
        "board_id":null,
        "cbs":[],
        "pending":[],
        "groups":{},
        "jobs":{},
        "current_cut":null,
        "tools":[],
        "config":{"task_goal":"synthetic legacy goal","editor_feedback":feedback},
        "runtime":{"editor_invalid_fallback":"none","max_pending_seconds":1.0,
            "hierarchy_auto":{"enabled":false}},
        "events":[]
    });
    sqlx::query("INSERT INTO chains(id,state) VALUES(?,?)")
        .bind("synthetic-python-agent")
        .bind(state.to_string())
        .execute(&pool)
        .await
        .unwrap();
    for index in 0..5 {
        let event = json!({"type":"runtime.configure","request_id":format!("fixture-{index}"),
            "sender_actor":"synthetic-owner","time":index});
        sqlx::query("INSERT INTO runtime_queue(id,chain_id,actor,event,status) VALUES(?,?,?,?,?)")
            .bind(format!("event-{index}"))
            .bind("synthetic-python-agent")
            .bind("synthetic-owner")
            .bind(event.to_string())
            .bind("pending")
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
}

async fn migrated_fixture(
    invalid_feedback: bool,
) -> (tempfile::TempDir, Store, String, std::path::PathBuf) {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-source.sqlite");
    let target = directory.path().join("rust-target.sqlite");
    python_config_fixture(&source, invalid_feedback).await;
    let report = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();
    (directory, store, report.target_chain_id, target)
}

async fn target_pool(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn restores_only_validated_config_and_replays_digest_receipt() {
    let (_directory, store, chain_id, target_path) = migrated_fixture(false).await;
    let expected = read_target_guard(&store, "synthetic-owner", &chain_id)
        .await
        .unwrap();
    let report = restore_legacy_config(
        &store,
        "synthetic-owner",
        &chain_id,
        "restore-once",
        &expected,
    )
    .await
    .unwrap();
    let names = report
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["editor_feedback", "task_goal"]);
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("synthetic legacy goal")
    );
    assert_eq!(report.replayed, false);

    let pool = target_pool(&target_path).await;
    let state: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&chain_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(
        state.pointer("/config/task_goal").and_then(Value::as_str),
        Some("synthetic legacy goal")
    );
    assert_eq!(
        state
            .pointer("/config/editor_feedback/max_corrections")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        state
            .pointer("/config/editor_feedback/feedback_chars")
            .and_then(Value::as_u64),
        Some(1024)
    );

    let replay = restore_legacy_config(
        &store,
        "synthetic-owner",
        &chain_id,
        "restore-once",
        &expected,
    )
    .await
    .unwrap();
    assert_eq!(replay.replayed, true);
    assert_eq!(
        replay.target_config_sha256_after,
        report.target_config_sha256_after
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM legacy_config_restore_receipts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let queue: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runtime_queue WHERE chain_id=? ORDER BY id")
            .bind(&chain_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(queue, vec!["legacy-quarantined"; 5]);
}

#[tokio::test]
async fn rejects_non_owner_target_edits_and_invalid_archived_values() {
    let (_directory, store, chain_id, target_path) = migrated_fixture(false).await;
    let expected = read_target_guard(&store, "synthetic-owner", &chain_id)
        .await
        .unwrap();
    assert!(
        restore_legacy_config(&store, "other-owner", &chain_id, "wrong-owner", &expected)
            .await
            .is_err()
    );
    let has_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='legacy_config_restore_receipts'")
        .fetch_one(&target_pool(&target_path).await)
        .await
        .unwrap();
    assert_eq!(
        has_receipts, 0,
        "unauthorized caller must not create receipt state"
    );

    let pool = target_pool(&target_path).await;
    let state_text: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&chain_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut state: Value = serde_json::from_str(&state_text).unwrap();
    state["config"]["task_goal"] = json!("host customized goal");
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(state.to_string())
        .bind(&chain_id)
        .execute(&pool)
        .await
        .unwrap();
    let edited_guard = read_target_guard(&store, "synthetic-owner", &chain_id)
        .await
        .unwrap();
    assert!(
        restore_legacy_config(
            &store,
            "synthetic-owner",
            &chain_id,
            "do-not-overwrite",
            &edited_guard
        )
        .await
        .is_err()
    );
    let unchanged: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&chain_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let unchanged: Value = serde_json::from_str(&unchanged).unwrap();
    assert_eq!(
        unchanged
            .pointer("/config/task_goal")
            .and_then(Value::as_str),
        Some("host customized goal")
    );

    let (_invalid_dir, invalid_store, invalid_chain, _invalid_path) = migrated_fixture(true).await;
    let invalid_guard = read_target_guard(&invalid_store, "synthetic-owner", &invalid_chain)
        .await
        .unwrap();
    assert!(
        restore_legacy_config(
            &invalid_store,
            "synthetic-owner",
            &invalid_chain,
            "invalid-source",
            &invalid_guard
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn offline_cli_audits_digests_then_restores_only_with_explicit_guard() {
    let (_directory, _store, chain_id, target_path) = migrated_fixture(false).await;
    let db = target_path.to_str().unwrap();
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    let audit = Command::new(binary)
        .args(["legacy-config-audit", db, &chain_id, "synthetic-owner"])
        .output()
        .unwrap();
    assert!(
        audit.status.success(),
        "audit CLI failed: {}",
        String::from_utf8_lossy(&audit.stderr)
    );
    let audit_text = String::from_utf8(audit.stdout).unwrap();
    assert!(!audit_text.contains("synthetic legacy goal"));
    let audit_json: Value = serde_json::from_str(&audit_text).unwrap();
    assert_eq!(audit_json["eligible"], true);
    assert_eq!(audit_json["fields"][0]["name"], "editor_feedback");
    assert_eq!(audit_json["fields"][1]["name"], "task_goal");
    let guard = &audit_json["target_guard"];
    let args = [
        "legacy-config-restore".to_owned(),
        db.to_owned(),
        chain_id.clone(),
        "synthetic-owner".to_owned(),
        "cli-idempotency-key".to_owned(),
        guard["run_state"].as_str().unwrap().to_owned(),
        guard["cut_seq"].as_u64().unwrap().to_string(),
        guard["control_epoch"].as_u64().unwrap().to_string(),
        guard["input_epoch"].as_u64().unwrap().to_string(),
        guard["config_sha256"].as_str().unwrap().to_owned(),
    ];
    let restored = Command::new(binary).args(args).output().unwrap();
    assert!(
        restored.status.success(),
        "restore CLI failed: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let restored_text = String::from_utf8(restored.stdout).unwrap();
    assert!(!restored_text.contains("synthetic legacy goal"));
    let restored_json: Value = serde_json::from_str(&restored_text).unwrap();
    assert_eq!(restored_json["replayed"], false);
    assert_eq!(restored_json["fields"][1]["name"], "task_goal");

    let pool = target_pool(&target_path).await;
    let queue: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runtime_queue WHERE chain_id=? ORDER BY id")
            .bind(&chain_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(queue, vec!["legacy-quarantined"; 5]);
}
