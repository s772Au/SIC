use serde_json::{Value, json};
use sic_core_rust::{
    board::{BoardActor, BoardService},
    core::{Core, OperationContext},
    legacy::LegacyRawSnapshot,
    legacy_upgrade::upgrade_active_python_agent,
    storage::Store,
    tool_store::ToolStore,
};
use sqlx::{
    Executor, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;
use tempfile::tempdir;

async fn python_fixture(path: &Path, with_active_objects: bool) {
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
    let mut tools = Vec::new();
    for index in 0..15 {
        let name = format!("python_tool_{index:02}");
        tools.push(json!({
            "name":name,
            "descriptor":format!("Synthetic description {index}"),
            "schema":{"type":"function","function":{
                "name":name,"description":format!("Synthetic description {index}"),
                "parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}
            }},
            "discover_acl":["legacy-owner"],
            "expand_acl":["legacy-owner"],
            "execute_acl":if index % 2 == 0 {json!(["legacy-owner"])} else {json!([])},
            "expanded":index % 3 == 0
        }));
    }
    let state = json!({
        "chain_id":"python-chain-active",
        "owners":["legacy-owner"],
        "creators":[],
        "run_state":"running",
        "board_id":"python-board-1",
        "cbs":[],
        "pending":[],
        "groups":{},
        "jobs":{},
        "current_cut":null,
        "tools":tools,
        "runtime":{"enabled":true,"max_cycles":99},
        "provider_inflight":null,
        "events":[{"type":"runtime-enabled"}]
    });
    sqlx::query("INSERT INTO chains VALUES(?,?)")
        .bind("python-chain-active")
        .bind(state.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let board = json!({
        "revision":7,
        "value":{
            "notice":"synthetic board snapshot",
            "tasks":{"task-open":{"claimant":"python-chain-active","status":"claimed","description":"Synthetic pending task"}}
        },
        "owner":"legacy-owner",
        "read_acl":["legacy-owner","legacy-reader"],
        "write_acl":["legacy-owner","legacy-writer"]
    });
    sqlx::query("INSERT INTO boards VALUES(?,?)")
        .bind("python-board-1")
        .bind(board.to_string())
        .execute(&pool)
        .await
        .unwrap();
    for (index, status) in ["pending", "running", "pending", "done", "failed"]
        .into_iter()
        .enumerate()
    {
        sqlx::query("INSERT INTO runtime_queue VALUES(?,?,?,?,?)")
            .bind(format!("queue-{index}"))
            .bind("python-chain-active")
            .bind("legacy-owner")
            .bind(json!({"type":"synthetic_event","ordinal":index}).to_string())
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO bridge_bindings VALUES(?,?,?,?,?,?)")
        .bind("legacy-owner")
        .bind("session-1")
        .bind("{\"active\":true}")
        .bind("lease-1")
        .bind(1000_f64)
        .bind(123_i64)
        .execute(&pool)
        .await
        .unwrap();
    // Keep the source shape compatible with the recognized bridge schema.
    sqlx::query("INSERT INTO bridge_turns VALUES(?,?,?,?)")
        .bind("legacy-owner")
        .bind("session-1")
        .bind("turn-1")
        .bind("pending")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO host_tool_hosts VALUES(?,?)")
        .bind("host-1")
        .bind("{\"status\":\"open\"}")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO host_tool_tasks VALUES(?,?)")
        .bind("task-1")
        .bind("{\"status\":\"pending\"}")
        .execute(&pool)
        .await
        .unwrap();
    if with_active_objects {
        sqlx::query("INSERT INTO objects VALUES(?,?,?,?)")
            .bind("source-1")
            .bind("source")
            .bind("{}")
            .bind("synthetic")
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn upgrades_current_active_shape_with_full_archive_and_quarantined_sidecars() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-source.sqlite");
    let destination = directory.path().join("rust-target.sqlite");
    python_fixture(&source, false).await;
    let source_before = std::fs::read(&source).unwrap();

    let report = upgrade_active_python_agent(&source, &destination)
        .await
        .unwrap();
    assert_eq!(report.original_chain_id, "python-chain-active");
    assert_eq!(report.original_owner_count, 1);
    assert_eq!(report.imported_board_count, 1);
    assert_eq!(report.quarantined_tool_count, 15);
    assert_eq!(report.quarantined_runtime_event_count, 5);
    assert_eq!(report.pending_or_running_runtime_event_count, 3);
    assert_eq!(report.archived_bridge_binding_count, 1);
    assert_eq!(report.archived_bridge_turn_count, 1);
    assert_eq!(report.archived_host_tool_host_count, 1);
    assert_eq!(report.archived_host_tool_task_count, 1);
    assert_eq!(report.target_agent_run_state, "stopped");
    assert!(!report.source_database_modified);
    assert!(!report.executable_legacy_work_resumed);
    assert_eq!(std::fs::read(&source).unwrap(), source_before);

    let store = Store::open(&destination).await.unwrap();
    let core = Core::new(store.clone());
    let context = OperationContext {
        actor: "legacy-owner".into(),
        chain_id: Some(report.target_chain_id.clone()),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.get", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(chain["result"]["run_state"], "stopped");
    assert_eq!(chain["result"]["owners"], json!(["legacy-owner"]));
    let migrated = core
        .operate("legacy.status", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(migrated["result"]["migrated"], true);
    assert_eq!(migrated["result"]["target_run_state"], "stopped");
    let pending = core
        .operate("legacy.queue", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(pending["result"]["items"].as_array().unwrap().len(), 5);

    let board_id = chain["result"]["board_id"].as_str().unwrap();
    let board_actor = BoardActor::from_authenticated_host(&report.target_chain_id).unwrap();
    let mut tx = store.begin().await.unwrap();
    let board_view = BoardService::read_latest_i_in_transaction(&mut tx, &board_actor, board_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let imported_value = &board_view["entries"][0]["content"];
    assert_eq!(imported_value["legacy_board_id"], "python-board-1");
    assert_eq!(imported_value["legacy_revision"], 7);
    assert_eq!(
        imported_value["legacy_acl"]["write_acl"],
        json!(["legacy-owner", "legacy-writer"])
    );
    assert_eq!(
        imported_value["value"]["tasks"]["task-open"]["status"],
        "claimed"
    );
    assert!(
        imported_value["disposition"]
            .as_str()
            .unwrap()
            .contains("not a live task")
    );
    let mut acl_tx = store.begin().await.unwrap();
    let board_acl: String = sqlx::query_scalar("SELECT acl_json FROM sic_boards WHERE board_id=?")
        .bind(board_id)
        .fetch_one(&mut *acl_tx)
        .await
        .unwrap();
    acl_tx.commit().await.unwrap();
    let board_acl: Value = serde_json::from_str(&board_acl).unwrap();
    assert_eq!(board_acl["readers"], json!([report.target_chain_id]));
    assert_eq!(board_acl["writers"], json!([]));
    assert_eq!(board_acl["claimers"], json!([]));
    assert_eq!(board_acl["managers"], json!([]));

    let tools = ToolStore::new(store.clone());
    assert!(
        tools
            .search_descriptions(&report.target_chain_id, "legacy-owner", "")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        tools
            .begin_execution(&report.target_chain_id, "legacy-owner", "python_tool_00")
            .await
            .is_err()
    );
    let status = core
        .operate("tools.status", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(status["result"]["catalog"], json!([]));
    assert_eq!(status["result"]["active_leases"], json!([]));

    let mut tx = store.begin().await.unwrap();
    let queue_rows = sqlx::query("SELECT status,event FROM runtime_queue ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(queue_rows.len(), 5);
    for row in queue_rows {
        let status: String = row.try_get("status").unwrap();
        let event: String = row.try_get("event").unwrap();
        assert_eq!(status, "legacy-quarantined");
        let wrapped: Value = serde_json::from_str(&event).unwrap();
        assert!(matches!(
            wrapped["original_status"].as_str(),
            Some("pending" | "running" | "done" | "failed")
        ));
        assert_eq!(
            wrapped["execution"],
            "disabled; row is not eligible for scheduling"
        );
    }
    let archive_row = sqlx::query("SELECT data,digest FROM objects WHERE id=?")
        .bind(&report.source_archive_object_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let archive_data: String = archive_row.try_get("data").unwrap();
    let archive_digest: String = archive_row.try_get("digest").unwrap();
    assert_eq!(
        sic_core_rust::storage::sha256(archive_data.as_bytes()),
        archive_digest
    );
    let archive: Value = serde_json::from_str(&archive_data).unwrap();
    assert_eq!(archive["archive_sha256"], report.source_archive_sha256);
    assert_eq!(archive["tables"].as_array().unwrap().len(), 10);
    let verified_archive: LegacyRawSnapshot = serde_json::from_value(archive.clone()).unwrap();
    verified_archive.verify().unwrap();
    assert_eq!(
        verified_archive
            .tables
            .iter()
            .find(|table| table.name == "runtime_queue")
            .unwrap()
            .rows
            .len(),
        5
    );
    let tools_row = sqlx::query("SELECT state FROM sic_dynamic_toolsets WHERE chain_id=?")
        .bind(&report.target_chain_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let tool_state_text: String = tools_row.try_get("state").unwrap();
    let tool_state: Value = serde_json::from_str(&tool_state_text).unwrap();
    assert_eq!(tool_state["tools"].as_object().unwrap().len(), 15);
    assert!(
        tool_state["tools"]["python_tool_00"]["descriptor"]["description"]
            .as_str()
            .unwrap()
            .contains("Synthetic description")
    );
    assert_eq!(tool_state["acl"]["execute"], json!([]));
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn refuses_populated_history_before_creating_destination_and_never_touches_source() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-active-with-object.sqlite");
    let destination = directory.path().join("should-not-exist.sqlite");
    python_fixture(&source, true).await;
    let before = std::fs::read(&source).unwrap();
    let error = upgrade_active_python_agent(&source, &destination)
        .await
        .unwrap_err();
    assert_eq!(error.code, "legacy_upgrade_refused");
    assert!(!destination.exists());
    assert_eq!(std::fs::read(&source).unwrap(), before);
}
