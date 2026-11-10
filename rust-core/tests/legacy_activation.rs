use serde_json::{Value, json};
use sic_core_rust::{
    board::{BoardActor, BoardService},
    core::{Core, OperationContext},
    legacy_activation::{
        activate_verified_owner_board, audit_quarantined_activation, start_activated_agent,
    },
    legacy_upgrade::upgrade_active_python_agent,
    storage::Store,
};
use sqlx::{
    Executor, Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;
use tempfile::tempdir;

const LEGACY_TOOLS: [&str; 15] = [
    "sic_source_read",
    "sic_source_search",
    "sic_operation_directory",
    "sic_hierarchy_legacy",
    "sic_memory_replay",
    "sic_hierarchy_expand",
    "sic_hierarchy_timeline",
    "sic_memory_page",
    "sic_memory_timeline",
    "sic_memory_locate",
    "sic_decision_update",
    "sic_socket_dispatch",
    "sic_socket_signal",
    "sic_agent_routes",
    "sic_collaboration_status",
];

async fn python_fixture(path: &Path, board_acl_matches_owner: bool) {
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
    ] {
        pool.execute(statement).await.unwrap();
    }
    let tools = LEGACY_TOOLS
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({
                "name":name,
                "descriptor":format!("Synthetic {name}"),
                "schema":{"type":"function","function":{
                    "name":name,
                    "description":format!("Synthetic {name}"),
                    "parameters":{"type":"object","properties":{"offset":{"type":"integer"}}}
                }},
                "discover_acl":["legacy-owner"],
                "expand_acl":["legacy-owner"],
                "execute_acl":["legacy-owner"],
                "expanded":index % 2 == 0
            })
        })
        .collect::<Vec<_>>();
    let state = json!({
        "chain_id":"python-active",
        "owners":["legacy-owner"],
        "creators":[],
        "run_state":"running",
        "board_id":"python-board",
        "cbs":[],
        "pending":[],
        "groups":{},
        "jobs":{},
        "current_cut":null,
        "tools":tools,
        "runtime":{"enabled":true},
        "events":[{"type":"runtime.configure","request_id":"fixture"}]
    });
    sqlx::query("INSERT INTO chains VALUES(?,?)")
        .bind("python-active")
        .bind(state.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let read_acl = if board_acl_matches_owner {
        json!(["legacy-owner"])
    } else {
        json!(["legacy-owner", "unmapped-reader"])
    };
    let board = json!({
        "revision":11,
        "value":{"status":"fixture board"},
        "owner":"legacy-owner",
        "read_acl":read_acl,
        "write_acl":["legacy-owner"]
    });
    sqlx::query("INSERT INTO boards VALUES(?,?)")
        .bind("python-board")
        .bind(board.to_string())
        .execute(&pool)
        .await
        .unwrap();
    for index in 0..5 {
        sqlx::query("INSERT INTO runtime_queue VALUES(?,?,?,?,?)")
            .bind(format!("event-{index}"))
            .bind("python-active")
            .bind("legacy-owner")
            .bind(json!({"type":"runtime.configure","request_id":index}).to_string())
            .bind("pending")
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn audits_and_activates_only_owner_equivalent_board_permissions() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python.sqlite");
    let target = directory.path().join("rust.sqlite");
    python_fixture(&source, true).await;
    let source_before = std::fs::read(&source).unwrap();
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), source_before);

    let store = Store::open(&target).await.unwrap();
    let audit = audit_quarantined_activation(&store, &upgrade.target_chain_id, "legacy-owner")
        .await
        .unwrap();
    assert!(audit.eligible);
    assert!(!audit.already_activated);
    assert_eq!(audit.board_acl_state, "restricted");
    assert_eq!(audit.quarantined_runtime_event_count, 5);
    assert_eq!(audit.pending_or_running_event_count, 5);
    assert!(!audit.runtime_events_replayed);
    assert_eq!(audit.native_tool_mappings.len(), 6);
    assert_eq!(audit.isolated_tools.len(), 9);
    assert!(audit.native_tool_mappings.iter().any(|mapping| {
        mapping.legacy_name == "sic_source_read"
            && mapping.rust_target.as_deref() == Some("sic_memory_page")
    }));
    assert!(audit.native_tool_mappings.iter().any(|mapping| {
        mapping.legacy_name == "sic_decision_update" && mapping.interface == "host_operation_only"
    }));
    assert!(
        audit_quarantined_activation(&store, &upgrade.target_chain_id, "other-owner")
            .await
            .is_err()
    );
    assert!(
        activate_verified_owner_board(
            &store,
            &upgrade.target_chain_id,
            "other-owner",
            audit.board_revision,
            "activate-owner-board"
        )
        .await
        .is_err()
    );
    assert!(audit.isolated_tools.iter().any(|mapping| {
        mapping.legacy_name == "sic_socket_dispatch" && mapping.rust_target.is_none()
    }));

    let actor = BoardActor::from_authenticated_host(&upgrade.target_chain_id).unwrap();
    let mut tx = store.begin().await.unwrap();
    let board_view = BoardService::read_latest_i_in_transaction(&mut tx, &actor, &audit.board_id)
        .await
        .unwrap();
    assert_eq!(board_view["tasks"], json!([]));
    let acl_text: String = sqlx::query_scalar("SELECT acl_json FROM sic_boards WHERE board_id=?")
        .bind(&audit.board_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let frozen_acl: Value = serde_json::from_str(&acl_text).unwrap();
    assert_eq!(frozen_acl["readers"], json!([upgrade.target_chain_id]));
    assert_eq!(frozen_acl["writers"], json!([]));
    assert_eq!(frozen_acl["claimers"], json!([]));
    assert_eq!(frozen_acl["managers"], json!([]));
    tx.commit().await.unwrap();

    let receipt = activate_verified_owner_board(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        audit.board_revision,
        "activate-owner-board",
    )
    .await
    .unwrap();
    assert_eq!(receipt.previous_board_revision, audit.board_revision);
    assert_eq!(receipt.activated_board_revision, audit.board_revision + 1);
    assert_eq!(
        receipt.granted_board_roles,
        ["read", "write", "claim", "manage"]
    );
    assert_eq!(receipt.quarantined_runtime_event_count, 5);
    assert_eq!(receipt.pending_or_running_event_count, 5);
    assert!(!receipt.runtime_events_replayed);
    assert!(!receipt.replayed);

    let replay = activate_verified_owner_board(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        audit.board_revision,
        "activate-owner-board",
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert!(
        activate_verified_owner_board(
            &store,
            &upgrade.target_chain_id,
            "other-owner",
            audit.board_revision,
            "activate-owner-board"
        )
        .await
        .is_err()
    );
    assert_eq!(
        replay.activation_audit_object_id,
        receipt.activation_audit_object_id
    );
    let after = audit_quarantined_activation(&store, &upgrade.target_chain_id, "legacy-owner")
        .await
        .unwrap();
    assert!(after.already_activated);
    assert_eq!(after.board_revision, audit.board_revision + 1);
    assert_eq!(after.quarantined_runtime_event_count, 5);

    let mut tx = store.begin().await.unwrap();
    let acl_text: String = sqlx::query_scalar("SELECT acl_json FROM sic_boards WHERE board_id=?")
        .bind(&audit.board_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let activated_acl: Value = serde_json::from_str(&acl_text).unwrap();
    for role in ["readers", "writers", "claimers", "managers"] {
        assert_eq!(activated_acl[role], json!([upgrade.target_chain_id]));
    }
    let queue = sqlx::query("SELECT status,event FROM runtime_queue WHERE chain_id=?")
        .bind(&upgrade.target_chain_id)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(queue.len(), 5);
    for row in queue {
        let status: String = row.try_get("status").unwrap();
        let event_text: String = row.try_get("event").unwrap();
        let event: Value = serde_json::from_str(&event_text).unwrap();
        assert_eq!(status, "legacy-quarantined");
        assert_eq!(event["original_status"], "pending");
        assert_eq!(
            event["execution"],
            "disabled; row is not eligible for scheduling"
        );
    }
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects WHERE kind='legacy_permission_activation'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn mismatched_legacy_acl_refuses_activation_without_target_mutation() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-mismatch.sqlite");
    let target = directory.path().join("rust-mismatch.sqlite");
    python_fixture(&source, false).await;
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();
    let error = audit_quarantined_activation(&store, &upgrade.target_chain_id, "legacy-owner")
        .await
        .unwrap_err();
    assert_eq!(error.code, "legacy_activation_refused");
    assert!(
        activate_verified_owner_board(
            &store,
            &upgrade.target_chain_id,
            "legacy-owner",
            3,
            "mismatched-acl"
        )
        .await
        .is_err()
    );
    let mut tx = store.begin().await.unwrap();
    let (revision, acl_text): (i64, String) =
        sqlx::query_as("SELECT revision,acl_json FROM sic_boards WHERE board_id=(SELECT board_id FROM chains WHERE id=?)")
            .bind(&upgrade.target_chain_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let acl: Value = serde_json::from_str(&acl_text).unwrap();
    assert_eq!(revision, 3);
    assert_eq!(acl["writers"], json!([]));
    let event_statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runtime_queue WHERE chain_id=?")
            .bind(&upgrade.target_chain_id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(event_statuses, vec!["legacy-quarantined"; 5]);
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn starts_only_activated_owner_route_and_replays_idempotently() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-start.sqlite");
    let target = directory.path().join("rust-start.sqlite");
    python_fixture(&source, true).await;
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();

    let non_owner = start_activated_agent(&store, &upgrade.target_chain_id, "other-owner", "start")
        .await
        .unwrap_err();
    assert_eq!(non_owner.code, "forbidden");
    let before_activation = start_activated_agent(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        "start-before-activation",
    )
    .await
    .unwrap_err();
    assert_eq!(before_activation.code, "legacy_activation_refused");

    activate_verified_owner_board(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        3,
        "activate-before-start",
    )
    .await
    .unwrap();
    let core = Core::new(store.clone());
    let owner_context = OperationContext {
        actor: "legacy-owner".into(),
        chain_id: Some(upgrade.target_chain_id.clone()),
        allow_write: true,
        ..OperationContext::default()
    };
    let started = core
        .operate(
            "legacy.activation.start",
            &owner_context,
            &json!({"idempotency_key":"start"}),
        )
        .await
        .unwrap();
    let receipt = started["result"].clone();
    assert_eq!(receipt["run_state"], "running");
    assert_eq!(receipt["runtime_events_replayed"], false);
    assert_eq!(receipt["quarantined_runtime_event_count"], 5);
    assert_eq!(receipt["pending_or_running_event_count"], 5);
    assert_eq!(receipt["replayed"], false);

    let ingested = core
        .operate(
            "source.ingest",
            &owner_context,
            &json!({"request":{"messages":[{"role":"user","content":"post-start source"}]}}),
        )
        .await
        .unwrap();
    assert_eq!(ingested["ok"], true);

    let replay = core
        .operate(
            "legacy.activation.start",
            &owner_context,
            &json!({"idempotency_key":"start"}),
        )
        .await
        .unwrap();
    assert_eq!(replay["result"]["replayed"], true);
    assert_eq!(
        replay["result"]["start_audit_object_id"],
        receipt["start_audit_object_id"]
    );
    let non_owner_context = OperationContext {
        actor: "other-owner".into(),
        ..owner_context.clone()
    };
    assert!(
        core.operate(
            "legacy.activation.start",
            &non_owner_context,
            &json!({"idempotency_key":"start"}),
        )
        .await
        .is_err()
    );

    let mut tx = store.begin().await.unwrap();
    let chain_state_text: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&upgrade.target_chain_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let chain_state: Value = serde_json::from_str(&chain_state_text).unwrap();
    assert_eq!(chain_state["run_state"], "running");
    let board_row = sqlx::query("SELECT revision,acl_json FROM sic_boards WHERE board_id=?")
        .bind(chain_state["board_id"].as_str().unwrap())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let board_revision: i64 = board_row.try_get("revision").unwrap();
    let board_acl_text: String = board_row.try_get("acl_json").unwrap();
    let board_acl: Value = serde_json::from_str(&board_acl_text).unwrap();
    assert_eq!(board_revision, 4);
    for role in ["readers", "writers", "claimers", "managers"] {
        assert_eq!(board_acl[role], json!([upgrade.target_chain_id]));
    }
    let queue_statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runtime_queue WHERE chain_id=? ORDER BY id")
            .bind(&upgrade.target_chain_id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(queue_statuses, vec!["legacy-quarantined"; 5]);
    let start_audits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE kind='legacy_agent_start'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(start_audits, 1);
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn refuses_start_if_new_material_exists() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-start-material.sqlite");
    let target = directory.path().join("rust-start-material.sqlite");
    python_fixture(&source, true).await;
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();
    activate_verified_owner_board(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        3,
        "activate-before-material-check",
    )
    .await
    .unwrap();
    let mut tx = store.begin().await.unwrap();
    let material = json!({"source_id":"newly-added"}).to_string();
    let digest = sic_core_rust::storage::sha256(material.as_bytes());
    sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
        .bind("unexpected-source")
        .bind("source")
        .bind(&material)
        .bind(digest)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let error = start_activated_agent(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        "start-after-material",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "legacy_activation_refused");
    let mut tx = store.begin().await.unwrap();
    let state_text: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&upgrade.target_chain_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(state["run_state"], "stopped");
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runtime_queue WHERE chain_id=?")
            .bind(&upgrade.target_chain_id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(statuses, vec!["legacy-quarantined"; 5]);
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn refuses_start_if_created_ipc_route_is_missing() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-start-no-route.sqlite");
    let target = directory.path().join("rust-start-no-route.sqlite");
    python_fixture(&source, true).await;
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();
    activate_verified_owner_board(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        3,
        "activate-before-route-check",
    )
    .await
    .unwrap();
    let mut tx = store.begin().await.unwrap();
    sqlx::query("DELETE FROM sic_rust_ipc_states WHERE namespace=?")
        .bind("sic-agent-workspace")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let error = start_activated_agent(
        &store,
        &upgrade.target_chain_id,
        "legacy-owner",
        "start-without-route",
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "legacy_activation_refused");
    let mut tx = store.begin().await.unwrap();
    let state_text: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&upgrade.target_chain_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(state["run_state"], "stopped");
    tx.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn core_activation_operations_enforce_actor_and_write_ceiling() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("python-core-activation.sqlite");
    let target = directory.path().join("rust-core-activation.sqlite");
    python_fixture(&source, true).await;
    let upgrade = upgrade_active_python_agent(&source, &target).await.unwrap();
    let store = Store::open(&target).await.unwrap();
    let core = Core::new(store.clone());
    let owner_read = OperationContext {
        actor: "legacy-owner".into(),
        chain_id: Some(upgrade.target_chain_id.clone()),
        allow_write: false,
        ..OperationContext::default()
    };
    let audit = core
        .operate("legacy.activation.audit", &owner_read, &json!({}))
        .await
        .unwrap();
    assert_eq!(audit["ok"], true);
    assert_eq!(audit["result"]["eligible"], true);

    let non_owner = OperationContext {
        actor: "other-owner".into(),
        chain_id: Some(upgrade.target_chain_id.clone()),
        allow_write: true,
        ..OperationContext::default()
    };
    let rejected_audit = core
        .operate("legacy.activation.audit", &non_owner, &json!({}))
        .await
        .unwrap_err();
    assert_eq!(rejected_audit.code, "forbidden");

    let apply_payload = json!({
        "expected_board_revision":audit["result"]["board_revision"],
        "idempotency_key":"core-activate"
    });
    let denied_write = core
        .operate("legacy.activation.apply", &owner_read, &apply_payload)
        .await
        .unwrap_err();
    assert_eq!(denied_write.code, "write_denied");
    let rejected_apply = core
        .operate("legacy.activation.apply", &non_owner, &apply_payload)
        .await
        .unwrap_err();
    assert_eq!(rejected_apply.code, "forbidden");

    let owner_write = OperationContext {
        allow_write: true,
        ..owner_read
    };
    let applied = core
        .operate("legacy.activation.apply", &owner_write, &apply_payload)
        .await
        .unwrap();
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["result"]["activated_board_revision"], 4);
    let replay = core
        .operate("legacy.activation.apply", &owner_write, &apply_payload)
        .await
        .unwrap();
    assert_eq!(replay["result"]["replayed"], true);
    store.close().await;
}
