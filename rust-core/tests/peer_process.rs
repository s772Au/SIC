use serde_json::{Value, json};
use sic_core_rust::{
    board::{BoardAcl, BoardActor, BoardService},
    core::{Core, OperationContext},
    peer_process::{self, HostProcessSpec},
    peer_tasks,
    storage::Store,
};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tempfile::tempdir;

fn node() -> Option<PathBuf> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

fn spec(executable: PathBuf, argv: &[&str], timeout: Duration) -> HostProcessSpec {
    let mut environment = BTreeMap::new();
    for key in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key).and_then(|v| v.into_string().ok()) {
            environment.insert(key.to_owned(), value);
        }
    }
    HostProcessSpec {
        executable,
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        environment,
        working_directory: None,
        timeout,
    }
}

fn context(actor: &str, chain_id: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain_id.map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}

async fn fixture(store: &Store) -> (Core, String) {
    let core = Core::new(store.clone());
    let route = core
        .operate("chain.create", &context("alice", None), &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tx = store.begin().await.unwrap();
    let encoded: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&route)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut state: Value = serde_json::from_str(&encoded).unwrap();
    // The launch verifier requires a complete, idle Agent with a current cut.
    // The empty worker selection is valid here and creates no external work.
    state["current_cut"] = json!("synthetic-current-cut");
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(state.to_string())
        .bind(&route)
        .execute(&mut *tx)
        .await
        .unwrap();
    peer_tasks::initialize_in_transaction(&mut tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sic_peer_tasks(task_id,actor,requester_chain_id,peer_chain_id,socket_id,checkpoint_id,request_id,message_id,lifecycle,state,claim_id,claimed_by,revision) VALUES('task-1','alice','requester',?,'socket','checkpoint','request','message','V','prepared','claim-1','alice',2)")
        .bind(&route).execute(&mut *tx).await.unwrap();
    let result = json!({"state":"prepared","ready_for_dsh":true,"peer_chain_id":route,"evidence":{"spawn_message_delivery":{"verified":true}}});
    sqlx::query("INSERT INTO sic_peer_task_prepares(task_id,prepare_id,actor,claim_id,fingerprint,running_revision,policy,state,result) VALUES('task-1','prepare-1','alice','claim-1','synthetic',1,'{}','prepared',?)")
        .bind(result.to_string()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    (core, route)
}

async fn set_state_field(store: &Store, route: &str, field: &str, value: Value) {
    let mut tx = store.begin().await.unwrap();
    let encoded: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(route)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut state: Value = serde_json::from_str(&encoded).unwrap();
    state[field] = value;
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(state.to_string())
        .bind(route)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn launch(
    store: &Store,
    route: &str,
    node: PathBuf,
    launch_id: &str,
    argv: &[&str],
    timeout: Duration,
) -> sic_core_rust::error::Result<peer_process::LaunchReceipt> {
    peer_process::launch_prepared_peer(
        store,
        "alice",
        route,
        "task-1",
        "claim-1",
        launch_id,
        &spec(node, argv, timeout),
    )
    .await
}

#[tokio::test]
async fn direct_child_exit_is_not_task_completion_and_replay_never_spawns() {
    let Some(node) = node() else {
        eprintln!("node is not installed; skipping executable-dependent test");
        return;
    };
    let dir = tempdir().unwrap();
    let store = Store::create(&dir.path().join("core.sqlite"))
        .await
        .unwrap();
    let (_core, route) = fixture(&store).await;
    assert!(
        peer_process::inspect_launch(&store, "alice", &route, "task-1", "claim-1", "launch-1")
            .await
            .unwrap()
            .is_none()
    );
    let result = launch(
        &store,
        &route,
        node.clone(),
        "launch-1",
        &["-e", "process.exit(7)"],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(result.state, "exited");
    assert_eq!(result.exit_code, Some(7));
    assert!(result.process_exited);
    let inspected =
        peer_process::inspect_launch(&store, "alice", &route, "task-1", "claim-1", "launch-1")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(inspected.state, "exited");
    assert_eq!(inspected.exit_code, Some(7));
    let same_spec = spec(
        node.clone(),
        &["-e", "process.exit(7)"],
        Duration::from_secs(5),
    );
    assert!(
        peer_process::inspect_launch_with_spec(
            &store, "alice", &route, "task-1", "claim-1", "launch-1", &same_spec
        )
        .await
        .unwrap()
        .is_some()
    );
    let changed_spec = spec(
        node.clone(),
        &["-e", "process.exit(0)"],
        Duration::from_secs(5),
    );
    assert!(
        peer_process::inspect_launch_with_spec(
            &store,
            "alice",
            &route,
            "task-1",
            "claim-1",
            "launch-1",
            &changed_spec
        )
        .await
        .is_err()
    );
    assert!(
        peer_process::inspect_launch(&store, "alice", &route, "task-1", "wrong", "launch-1")
            .await
            .is_err()
    );
    assert!(
        peer_process::inspect_launch(&store, "mallory", &route, "task-1", "claim-1", "launch-1")
            .await
            .is_err()
    );
    set_state_field(&store, &route, "run_state", json!("stopped")).await;
    let replay = launch(
        &store,
        &route,
        node.clone(),
        "launch-1",
        &["-e", "process.exit(7)"],
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, "exited");
    assert!(
        launch(
            &store,
            &route,
            node,
            "launch-1",
            &["-e", "process.exit(0)"],
            Duration::from_secs(5)
        )
        .await
        .is_err()
    );
    let mut tx = store.begin().await.unwrap();
    let task_state: String =
        sqlx::query_scalar("SELECT state FROM sic_peer_tasks WHERE task_id='task-1'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(task_state, "prepared");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn host_selected_working_directory_is_bound_to_the_launch_receipt() {
    let Some(node) = node() else { return };
    let dir = tempdir().unwrap();
    let store = Store::create(&dir.path().join("cwd.sqlite")).await.unwrap();
    let (_core, route) = fixture(&store).await;
    let mut command = spec(
        node,
        &[
            "-e",
            "process.exit(process.cwd() === process.env.EXPECTED_CWD ? 0 : 9)",
        ],
        Duration::from_secs(5),
    );
    let cwd = dir.path().to_path_buf();
    command
        .environment
        .insert("EXPECTED_CWD".into(), cwd.to_string_lossy().into_owned());
    command.working_directory = Some(cwd.clone());
    let result = peer_process::launch_prepared_peer(
        &store,
        "alice",
        &route,
        "task-1",
        "claim-1",
        "launch-cwd",
        &command,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    command.working_directory = Some(cwd.join("does-not-exist"));
    assert!(
        peer_process::launch_prepared_peer(
            &store,
            "alice",
            &route,
            "task-1",
            "claim-1",
            "launch-cwd",
            &command
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn two_store_connections_only_one_can_obtain_launch_right() {
    let Some(node) = node() else {
        eprintln!("node is not installed; skipping executable-dependent test");
        return;
    };
    let dir = tempdir().unwrap();
    let path = dir.path().join("core.sqlite");
    let first = Store::create(&path).await.unwrap();
    let (_core, route) = fixture(&first).await;
    let second = Store::open(&path).await.unwrap();
    let args = ["-e", "setTimeout(()=>process.exit(0),100)"];
    let one = spec(node.clone(), &args, Duration::from_secs(5));
    let two = spec(node.clone(), &args, Duration::from_secs(5));
    let (a, b) = tokio::join!(
        peer_process::launch_prepared_peer(
            &first,
            "alice",
            &route,
            "task-1",
            "claim-1",
            "launch-race",
            &one
        ),
        peer_process::launch_prepared_peer(
            &second,
            "alice",
            &route,
            "task-1",
            "claim-1",
            "launch-race",
            &two
        ),
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.replayed, b.replayed);
    let final_receipt = launch(
        &first,
        &route,
        node,
        "launch-race",
        &args,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(final_receipt.replayed);
    assert_eq!(final_receipt.state, "exited");
}

#[tokio::test]
async fn timeout_is_unknown_and_explicit_reconciliation_never_restarts() {
    let Some(node) = node() else {
        eprintln!("node is not installed; skipping executable-dependent test");
        return;
    };
    let dir = tempdir().unwrap();
    let store = Store::create(&dir.path().join("core.sqlite"))
        .await
        .unwrap();
    let (_core, route) = fixture(&store).await;
    let result = launch(
        &store,
        &route,
        node.clone(),
        "launch-timeout",
        &["-e", "setTimeout(()=>process.exit(0),5000)"],
        Duration::from_millis(50),
    )
    .await
    .unwrap();
    assert_eq!(result.state, "unknown");
    assert!(!result.process_exited);
    let recovered =
        peer_process::mark_interrupted_unknown(&store, "alice", &route, "task-1", "launch-timeout")
            .await
            .unwrap();
    assert_eq!(recovered.state, "unknown");
    let replay = launch(
        &store,
        &route,
        node,
        "launch-timeout",
        &["-e", "setTimeout(()=>process.exit(0),5000)"],
        Duration::from_millis(50),
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, "unknown");
}

#[tokio::test]
async fn current_core_readiness_owner_claim_and_shell_constraints_are_enforced() {
    let Some(node) = node() else {
        eprintln!("node is not installed; skipping executable-dependent test");
        return;
    };
    let dir = tempdir().unwrap();
    let store = Store::create(&dir.path().join("core.sqlite"))
        .await
        .unwrap();
    let (_core, route) = fixture(&store).await;
    let normal = spec(
        node.clone(),
        &["-e", "process.exit(0)"],
        Duration::from_secs(1),
    );
    assert!(
        peer_process::launch_prepared_peer(
            &store, "mallory", &route, "task-1", "claim-1", "nonowner", &normal
        )
        .await
        .is_err()
    );
    assert!(
        peer_process::launch_prepared_peer(
            &store,
            "alice",
            &route,
            "task-1",
            "wrong-claim",
            "wrong-claim",
            &normal
        )
        .await
        .is_err()
    );
    set_state_field(&store, &route, "run_state", json!("stopped")).await;
    assert!(
        launch(
            &store,
            &route,
            node.clone(),
            "stopped",
            &["-e", "process.exit(0)"],
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
    set_state_field(&store, &route, "run_state", json!("running")).await;
    set_state_field(&store, &route, "worker_claim", json!("busy")).await;
    assert!(
        launch(
            &store,
            &route,
            node.clone(),
            "busy",
            &["-e", "process.exit(0)"],
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
    set_state_field(&store, &route, "worker_claim", Value::Null).await;
    let batch = dir.path().join("blocked.bat");
    std::fs::write(&batch, "echo blocked").unwrap();
    let script = HostProcessSpec {
        executable: batch,
        argv: vec![],
        environment: BTreeMap::new(),
        working_directory: None,
        timeout: Duration::from_secs(1),
    };
    assert!(
        peer_process::launch_prepared_peer(
            &store, "alice", &route, "task-1", "claim-1", "script", &script
        )
        .await
        .is_err()
    );
    if let Some(root) = std::env::var_os("SystemRoot") {
        let cmd = PathBuf::from(root).join("System32").join("cmd.exe");
        if cmd.is_file() {
            assert!(
                launch(
                    &store,
                    &route,
                    cmd,
                    "shell",
                    &["/c", "exit 0"],
                    Duration::from_secs(1)
                )
                .await
                .is_err()
            );
        }
    }
    let mut tx = store.begin().await.unwrap();
    sqlx::query("UPDATE sic_peer_task_prepares SET result=? WHERE task_id='task-1'").bind(json!({"state":"prepared","ready_for_dsh":false,"peer_chain_id":route,"evidence":{"spawn_message_delivery":{"verified":false}}}).to_string()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        launch(
            &store,
            &route,
            node,
            "not-ready",
            &["-e", "process.exit(0)"],
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn current_source_dependencies_and_board_acl_are_revalidated() {
    let Some(node) = node() else {
        eprintln!("node is not installed; skipping executable-dependent test");
        return;
    };
    let dir = tempdir().unwrap();
    let store = Store::create(&dir.path().join("core.sqlite"))
        .await
        .unwrap();
    let (_core, route) = fixture(&store).await;
    let node2 = node.clone();
    let mut tx = store.begin().await.unwrap();
    let encoded: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&route)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut state: Value = serde_json::from_str(&encoded).unwrap();
    state["groups"] = json!({"main":["missing-source"]});
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(state.to_string())
        .bind(&route)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        launch(
            &store,
            &route,
            node2,
            "missing-source",
            &["-e", "process.exit(0)"],
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );

    let mut tx = store.begin().await.unwrap();
    let acl = BoardAcl::new(["other"], ["other"], ["other"], ["other"]).unwrap();
    BoardService::configure_from_host_in_transaction(
        &mut tx,
        "board-denied",
        &BoardActor::from_authenticated_host(&route).unwrap(),
        acl,
    )
    .await
    .unwrap();
    let encoded: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&route)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut state: Value = serde_json::from_str(&encoded).unwrap();
    state["groups"] = json!({});
    state["board_id"] = json!("board-denied");
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(state.to_string())
        .bind(&route)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        launch(
            &store,
            &route,
            node,
            "revoked-board",
            &["-e", "process.exit(0)"],
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
}
