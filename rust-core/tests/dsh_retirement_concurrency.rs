use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use sic_core_rust::{
    core::Core,
    dsh_bridge::{CoreRuntime, DshBridge, DshBridgeConfig},
    storage::Store,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
struct MockProvider {
    calls: AtomicUsize,
    worker_started: Notify,
    release_worker: Notify,
    fail_worker: bool,
}

async fn complete(
    State(provider): State<Arc<MockProvider>>,
    Json(_request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let call = provider.calls.fetch_add(1, Ordering::SeqCst);
    if call == 0 {
        return (
            StatusCode::OK,
            Json(json!({
                "model":"mock-model",
                "usage":{"prompt_tokens":10,"completion_tokens":4},
                "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":"editor-call","type":"function","function":{
                        "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"
                    }}]}}]
            })),
        );
    }

    provider.worker_started.notify_one();
    if provider.fail_worker {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"message":"synthetic worker failure"}})),
        );
    }
    provider.release_worker.notified().await;
    (
        StatusCode::OK,
        Json(json!({
            "model":"mock-model",
            "usage":{"prompt_tokens":12,"completion_tokens":5},
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"synthetic worker completion"}}]
        })),
    )
}

fn headers(session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-sic-bridge-session",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

fn request(text: &str) -> Value {
    json!({
        "model":"sic-worker",
        "messages":[{"role":"user","content":text}],
        "stream":true,
        "stream_options":{"include_usage":true},
        "tools":[],
        "tool_choice":"auto",
        "max_tokens":4096
    })
}

fn model_config(endpoint: String) -> Value {
    let profile = json!({
        "model":"openai/mock-model",
        "api_base":endpoint,
        "api_key_env":"LOCAL_TEST_FIXTURE",
        "tokenizer_model":"host-o200k-surrogate",
        "tokenizer_status":"surrogate-not-provider-native",
        "context_window":1_000_000,
        "max_input_tokens":1_000_000,
        "max_output_tokens":393_216,
        "default_output_tokens":4096,
        "timeout_seconds":10
    });
    json!({"roles":{"worker":profile,"editor":profile}})
}

fn bridge_config() -> DshBridgeConfig {
    DshBridgeConfig {
        model_alias: "sic-worker".into(),
        fixed_max_tokens: Some(4096),
        max_worker_calls_per_turn: 8,
        max_host_tasks_per_response: 4,
        max_harness_tools: 64,
        fixed_reasoning_effort: None,
    }
}

async fn start_provider(provider: Arc<MockProvider>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(provider),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}/v1"), server)
}

async fn open_store(path: &Path) -> Store {
    Store::create(path).await.unwrap()
}

async fn new_bridge(store: Store, endpoint: String) -> (Core, Arc<DshBridge>) {
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        model_config(endpoint),
        Arc::new(|_| Ok("fixture-only-key".into())),
    )
    .unwrap();
    let bridge = Arc::new(
        DshBridge::new(
            store,
            Arc::new(CoreRuntime::new(core.clone())),
            bridge_config(),
        )
        .await
        .unwrap(),
    );
    (core, bridge)
}

#[tokio::test]
async fn retirement_waits_for_inflight_provider_turn_then_rejects_successful_turn() {
    let provider = Arc::new(MockProvider::default());
    let (endpoint, server) = start_provider(provider.clone()).await;
    let store = Store::memory().await.unwrap();
    let (_core, bridge) = new_bridge(store, endpoint).await;

    let active_bridge = bridge.clone();
    let handle = tokio::spawn(async move {
        active_bridge
            .handle("owner", &headers("inflight"), &request("finish this task"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), provider.worker_started.notified())
        .await
        .expect("Worker provider request did not start");

    let status = bridge.session_status("owner", "inflight").await.unwrap();
    assert_eq!(status["state"], "running");
    let fingerprint = status["active_fingerprint"].as_str().unwrap().to_owned();
    let retirement_bridge = bridge.clone();
    let mut retirement = tokio::spawn(async move {
        retirement_bridge
            .retire_unresolved_session(
                "owner",
                "inflight",
                &fingerprint,
                "operator inspected in-flight request",
            )
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(75), &mut retirement)
            .await
            .is_err(),
        "retirement must not overtake the in-flight session gate"
    );
    provider.release_worker.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("bridge turn did not finish")
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8(response)
            .unwrap()
            .contains("synthetic worker completion")
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);

    let retired = tokio::time::timeout(Duration::from_secs(5), &mut retirement)
        .await
        .expect("retirement did not resume after the turn")
        .unwrap()
        .unwrap_err();
    assert_eq!(retired.code, "retirement_state_changed");
    assert_eq!(
        bridge.session_status("owner", "inflight").await.unwrap()["retired"],
        false
    );
    server.abort();
}

#[tokio::test]
async fn retired_indeterminate_turn_survives_bridge_restart_without_provider_replay() {
    let provider = Arc::new(MockProvider {
        fail_worker: true,
        ..MockProvider::default()
    });
    let (endpoint, server) = start_provider(provider.clone()).await;
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("retirement-restart.sqlite");
    let store = open_store(&database).await;
    let (core, bridge) = new_bridge(store.clone(), endpoint.clone()).await;

    assert!(
        bridge
            .handle("owner", &headers("restart-me"), &request("first attempt"))
            .await
            .is_err()
    );
    let status = bridge.session_status("owner", "restart-me").await.unwrap();
    assert_eq!(status["state"], "indeterminate");
    let fingerprint = status["active_fingerprint"].as_str().unwrap().to_owned();
    let phase = status["phase"].as_str().unwrap().to_owned();
    let before_retirement = provider.calls.load(Ordering::SeqCst);

    assert!(
        bridge
            .retire_unresolved_session(
                "other-actor",
                "restart-me",
                &fingerprint,
                "operator inspected failure",
            )
            .await
            .is_err()
    );
    assert_eq!(
        bridge.session_status("owner", "restart-me").await.unwrap()["retired"],
        false
    );
    let retirement = bridge
        .retire_unresolved_session(
            "owner",
            "restart-me",
            &fingerprint,
            "operator inspected failure",
        )
        .await
        .unwrap();
    assert_eq!(retirement["replayed"], false);
    assert_eq!(retirement["original_state"], "indeterminate");

    drop(bridge);
    drop(core);
    store.close().await;
    let reopened = Store::open(&database).await.unwrap();
    let (_reopened_core, reopened_bridge) = new_bridge(reopened, endpoint).await;

    let replay = reopened_bridge
        .retire_unresolved_session(
            "owner",
            "restart-me",
            &fingerprint,
            "operator inspected failure",
        )
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["original_phase"], phase);
    assert_eq!(
        reopened_bridge
            .session_status("owner", "restart-me")
            .await
            .unwrap()["retired"],
        true
    );
    let rejected = reopened_bridge
        .handle(
            "owner",
            &headers("restart-me"),
            &request("retry after restart"),
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "session_retired");
    assert_eq!(provider.calls.load(Ordering::SeqCst), before_retirement);
    server.abort();
}
