use std::{fs, process::Command};

fn execute(value: &serde_json::Value) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    fs::write(&config, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["watch", "--config"])
        .arg(&config)
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap()
}

#[test]
fn watch_requires_explicit_opt_in_and_does_not_need_a_bearer_to_refuse() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    for enabled in [Some(false), None] {
        if let Some(enabled) = enabled {
            value["enabled"] = enabled.into();
        } else {
            value.as_object_mut().unwrap().remove("enabled");
        }
        let output = execute(&value);
        assert_eq!(output.status.code(), Some(2));
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "watch_not_enabled");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn watch_rejects_unknown_fields_and_invalid_host_config_before_claiming() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    value["enabled"] = true.into();
    // Example paths intentionally do not exist; this must fail before the
    // missing bearer is examined or a task can be claimed.
    let output = execute(&value);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "invalid_configuration");

    value["unexpected"] = "ignored by no one".into();
    let output = execute(&value);
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "invalid_configuration");
}
fn execute_ack(value: &serde_json::Value, task_id: &str) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    fs::write(&config, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["ack", "--config"])
        .arg(&config)
        .args(["--task-id", task_id])
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap()
}

#[test]
fn ack_requires_config_and_task_id() {
    let output = Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["ack", "--config", "missing.json"])
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "usage");
}

#[test]
fn ack_reports_a_missing_store_without_reading_a_bearer() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    // Paths in the packaged example do not exist here. The acknowledgement must
    // fail on storage, not on the (unset) provider bearer.
    let output = execute_ack(&value, "task-fixture");
    assert_eq!(output.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "storage_unavailable");

    value["supervisor"]["db_path"] = serde_json::json!("relative-not-a-file.sqlite");
    let output = execute_ack(&value, "task-fixture");
    assert_eq!(output.status.code(), Some(1));
}
fn execute_ack_with_config(value: &serde_json::Value, task_id: &str) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    fs::write(&config, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["ack", "--config"])
        .arg(&config)
        .args(["--task-id", task_id])
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap()
}

fn execute_status(value: &serde_json::Value) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    fs::write(&config, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["status", "--config"])
        .arg(&config)
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap()
}

#[test]
fn status_lists_quarantine_rows_and_accepts_an_empty_ledger() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("sic.sqlite");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = sic_core_rust::storage::Store::create(&db).await.unwrap();
        let mut tx = store.begin().await.unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sic_peer_watch_governance (
                namespace TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL,
                failures INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('cooldown','quarantined')),
                until_ms INTEGER NOT NULL, reason TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY(namespace,task_id))",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sic_peer_watch_governance(namespace,task_id,revision,failures,state,until_ms,reason) VALUES('cli-watch','stuck-task',2,3,'quarantined',0,'supervisor_outcome_unknown')")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        drop(store);
    });

    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    value["namespace"] = serde_json::json!("cli-watch");
    value["supervisor"]["db_path"] = serde_json::json!(db.to_string_lossy());

    let output = execute_status(&value);
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["state"], "watch_status");
    assert_eq!(report["namespace"], "cli-watch");
    assert_eq!(report["governance"][0]["task_id"], "stuck-task");
    assert_eq!(report["governance"][0]["state"], "quarantined");
    assert_eq!(report["governance"][0]["failures"], 3);
    assert_eq!(
        report["governance"][0]["reason"],
        "supervisor_outcome_unknown"
    );

    let empty_dir = tempfile::tempdir().unwrap();
    let empty_db = empty_dir.path().join("sic.sqlite");
    runtime.block_on(async {
        drop(
            sic_core_rust::storage::Store::create(&empty_db)
                .await
                .unwrap(),
        );
    });
    value["supervisor"]["db_path"] = serde_json::json!(empty_db.to_string_lossy());
    let output = execute_status(&value);
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["governance"], serde_json::json!([]));
}

#[test]
fn ack_clears_a_real_quarantine_ledger_and_reports_repeat_calls() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("sic.sqlite");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store = sic_core_rust::storage::Store::create(&db).await.unwrap();
        let mut tx = store.begin().await.unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sic_peer_watch_governance (
                namespace TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL,
                failures INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('cooldown','quarantined')),
                until_ms INTEGER NOT NULL, reason TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY(namespace,task_id))",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sic_peer_watch_governance(namespace,task_id,revision,failures,state,until_ms,reason) VALUES('cli-watch','stuck-task',2,3,'quarantined',0,'fixture')")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        drop(store);
    });

    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    value["namespace"] = serde_json::json!("cli-watch");
    value["supervisor"]["db_path"] = serde_json::json!(db.to_string_lossy());

    let first = execute_ack_with_config(&value, "stuck-task");
    assert_eq!(first.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["state"], "acknowledged");
    assert_eq!(report["cleared"], true);

    let second = execute_ack_with_config(&value, "stuck-task");
    assert_eq!(second.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["cleared"], false);
}
