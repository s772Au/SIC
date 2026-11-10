use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    peer_runner_client::{HostPermissions, PeerRunnerClient},
    peer_supervisor::{self, SupervisorConfig},
    peer_tasks,
    server::{self, HostAuth},
    storage::Store,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tempfile::tempdir;
use tokio::task::JoinHandle;

const ACTOR: &str = "supervisor-test-owner";
const TOKEN: &str = "loopback-test-token";
const CLAIM: &str = "already-claimed-test";

#[derive(Clone)]
struct ProviderCounter(Arc<AtomicUsize>);

async fn mock_provider(
    State(counter): State<ProviderCounter>,
    Json(_request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    counter.0.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({
            "model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":1,"completion_tokens":1},
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"fixture"}}]
        })),
    )
}

fn context(actor: &str, chain_id: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain_id.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

fn node_executable() -> Option<PathBuf> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    config: Value,
    peer: String,
    task_id: String,
    sentinel: PathBuf,
    provider_calls: Arc<AtomicUsize>,
    _provider_server: JoinHandle<()>,
    core_server: JoinHandle<()>,
}

async fn fixture() -> Fixture {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("supervisor.sqlite");
    let store = Store::create(&db_path).await.unwrap();

    let provider_calls = Arc::new(AtomicUsize::new(0));
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_addr = provider_listener.local_addr().unwrap();
    let provider_state = ProviderCounter(provider_calls.clone());
    let provider_server = tokio::spawn(async move {
        axum::serve(
            provider_listener,
            Router::new()
                .route("/v1/chat/completions", post(mock_provider))
                .with_state(provider_state),
        )
        .await
        .unwrap();
    });
    let profile = json!({
        "model":"openai/deepseek-v4.1-flash-eco",
        "api_base":format!("http://{provider_addr}/v1"),
        "api_key_env":"LOCAL_SUPERVISOR_FIXTURE",
        "tokenizer_model":"host-o200k-surrogate",
        "tokenizer_status":"fixture",
        "context_window":1000000,
        "max_input_tokens":1000000,
        "max_output_tokens":65536,
        "default_output_tokens":4096,
        "timeout_seconds":5
    });
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-only-key".to_owned())),
    )
    .unwrap();
    let requester = core
        .operate("chain.create", &context(ACTOR, None), &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let peer = core
        .operate("chain.create", &context(ACTOR, None), &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let task_id = "claimed-without-launch".to_owned();
    let mut tx = store.begin().await.unwrap();
    peer_tasks::record_spawn_in_transaction(
        &mut tx,
        &task_id,
        ACTOR,
        &requester,
        &peer,
        "synthetic-socket",
        "synthetic-checkpoint",
        "synthetic-request",
        "synthetic-message",
        "P",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let auth = HostAuth::new(TOKEN, ACTOR, true, true, false).unwrap();
    let core_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core_addr = core_listener.local_addr().unwrap();
    let core_server = tokio::spawn(async move {
        axum::serve(core_listener, server::router(core, auth))
            .await
            .unwrap();
    });
    let core_base_url = format!("http://{core_addr}");
    let client = PeerRunnerClient::new(
        &core_base_url,
        TOKEN,
        ACTOR,
        HostPermissions::new(true, true, false),
    )
    .unwrap();
    let claimed = client
        .operate(
            "peer.task.claim",
            Some(&requester),
            &json!({"task_id":task_id,"claim_id":CLAIM}),
        )
        .await
        .unwrap();
    assert_eq!(claimed["result"]["state"], "claimed");
    assert_eq!(claimed["result"]["execution_permit"], true);
    assert_eq!(claimed["result"]["replayed"], false);

    let node =
        node_executable().expect("Node fixture is required for the no-launch sentinel assertion");
    let state_dir = dir.path().join("state");
    let home = dir.path().join("dsh-home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let sentinel = dir.path().join("node-was-started.txt");
    let cli = dir.path().join("dsh-cli.js");
    let sentinel_js = serde_json::to_string(&sentinel.to_string_lossy()).unwrap();
    std::fs::write(
        &cli,
        format!("require('fs').writeFileSync({sentinel_js}, 'started');\n"),
    )
    .unwrap();
    let config = json!({
        "db_path":db_path,
        "core_base_url":core_base_url,
        "bridge_base_url":"http://127.0.0.1:1/v1",
        "bearer_env":"SUPERVISOR_FIXTURE_TOKEN",
        "actor":ACTOR,
        "requester_chain_id":requester,
        "peer_chain_id":peer,
        "task_id":task_id,
        "reply_socket_id":"synthetic-reply-socket",
        "claim_id":CLAIM,
        "prepare_id":"synthetic-prepare",
        "launch_id":"synthetic-launch",
        "reply_id":"synthetic-reply",
        "policy":{
            "context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":1,
            "max_units_per_wb":64,"level_batch_sizes":{},"max_level":1,"flush":true,
            "max_actions":32,"max_model_calls":4,"editor_mode":"provider","program_rules":[]
        },
        "dsh_executable":node,
        "dsh_cli_js":cli,
        "dsh_home":home,
        "dsh_profile":"fixture",
        "working_directory":work,
        "state_directory":state_dir,
        "model_alias":"sic-fixture",
        "max_tokens":1024,
        "timeout_seconds":10,
        "api_timeout_seconds":3,
        "prices":{"input":0.0,"output":0.0,"cache_read":0.0,"cache_write":0.0},
        "allowed_tools":["sic_tool_search"],
        "prompt":"synthetic supervisor recovery fixture"
    });
    Fixture {
        _dir: dir,
        store,
        config,
        peer,
        task_id,
        sentinel,
        provider_calls,
        _provider_server: provider_server,
        core_server,
    }
}

#[tokio::test]
async fn reused_claim_without_launch_intent_stops_and_never_calls_child_or_provider() {
    let fixture = fixture().await;
    let cfg: SupervisorConfig = serde_json::from_value(fixture.config.clone()).unwrap();
    let error = peer_supervisor::run(&cfg, TOKEN.to_owned())
        .await
        .err()
        .unwrap();
    assert_eq!(error.0, "claim_already_used");
    assert!(
        !fixture.sentinel.exists(),
        "Node sentinel means the child was launched"
    );
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    assert!(
        sic_core_rust::peer_process::inspect_launch(
            &fixture.store,
            ACTOR,
            &fixture.peer,
            &fixture.task_id,
            CLAIM,
            "synthetic-launch"
        )
        .await
        .unwrap()
        .is_none()
    );
    fixture.core_server.abort();
    fixture._provider_server.abort();
}

#[tokio::test]
async fn invalid_actor_route_and_host_configuration_do_not_call_provider() {
    let fixture = fixture().await;
    let mut actor_config = fixture.config.clone();
    actor_config["actor"] = json!("other-actor");
    let actor_cfg: SupervisorConfig = serde_json::from_value(actor_config).unwrap();
    assert_eq!(
        peer_supervisor::run(&actor_cfg, TOKEN.to_owned())
            .await
            .err()
            .unwrap()
            .0,
        "claim_unconfirmed"
    );

    let mut route_config = fixture.config.clone();
    route_config["requester_chain_id"] = json!("missing-requester-route");
    let route_cfg: SupervisorConfig = serde_json::from_value(route_config).unwrap();
    assert_eq!(
        peer_supervisor::run(&route_cfg, TOKEN.to_owned())
            .await
            .err()
            .unwrap()
            .0,
        "claim_unconfirmed"
    );

    let mut invalid_config = fixture.config.clone();
    invalid_config["max_tokens"] = json!(0);
    let invalid_cfg: SupervisorConfig = serde_json::from_value(invalid_config).unwrap();
    assert_eq!(
        peer_supervisor::run(&invalid_cfg, TOKEN.to_owned())
            .await
            .err()
            .unwrap()
            .0,
        "invalid_configuration"
    );

    assert!(!fixture.sentinel.exists());
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    fixture.core_server.abort();
    fixture._provider_server.abort();
}
