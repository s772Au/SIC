pub mod core {
    pub use sic_core_rust::core::*;
}
pub mod storage {
    pub use sic_core_rust::storage::*;
}
#[path = "../src/dsh_bridge.rs"]
mod dsh_bridge;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::post,
};
use dsh_bridge::{ActorResolver, CoreRuntime, DshBridge, DshBridgeConfig, router as bridge_router};
use serde_json::{Value, json};
use sic_core_rust::{core::Core, storage::Store};
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct ProviderFixture {
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn completion(
    State(fixture): State<ProviderFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request.clone());
        requests.len()
    };
    let (finish_reason, message) = match index {
        1 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{
                "id":"editor-call-1","type":"function","function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{\"P\":[\"integration CB fact\"]}}"}
            }]}),
        ),
        2 => worker_tool_call(
            "worker-search-1",
            "sic_tool_search",
            json!({"query":"calculator"}),
        ),
        3 => worker_tool_call(
            "worker-expand-1",
            "sic_tool_expand",
            json!({"name":"calculator"}),
        ),
        4 => worker_tool_call(
            "worker-host-1",
            "sic_tool_call",
            json!({"name":"calculator","arguments":{"a":7,"b":5}}),
        ),
        5 => (
            "stop",
            json!({"role":"assistant","content":"Calculator returned 12; integration complete."}),
        ),
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"message":"unexpected fixture call"}})),
            );
        }
    };
    (
        StatusCode::OK,
        Json(
            json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":11,"completion_tokens":7},
            "choices":[{"finish_reason":finish_reason,"message":message}]}),
        ),
    )
}

fn worker_tool_call(id: &str, name: &str, arguments: Value) -> (&'static str, Value) {
    (
        "tool_calls",
        json!({"role":"assistant","content":null,"tool_calls":[{
            "id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}
        }]}),
    )
}

fn profile(endpoint: String) -> Value {
    json!({
        "model":"openai/deepseek-v4.1-flash-eco",
        "api_base":endpoint,
        "api_key_env":"LOCAL_TEST_FIXTURE",
        "tokenizer_model":"host-o200k-surrogate",
        "tokenizer_status":"surrogate-not-provider-native",
        "context_window":1_000_000,
        "max_input_tokens":1_000_000,
        "max_output_tokens":393_216,
        "default_output_tokens":4096,
        "timeout_seconds":10,
        "reasoning_effort":"high"
    })
}

fn bridge_config() -> DshBridgeConfig {
    DshBridgeConfig {
        model_alias: "sic-worker".into(),
        fixed_max_tokens: Some(4096),
        max_worker_calls_per_turn: 8,
        max_host_tasks_per_response: 4,
        max_harness_tools: 64,
        fixed_reasoning_effort: Some("high".into()),
    }
}

fn provider_config(endpoint: String) -> Value {
    let configured = profile(endpoint);
    json!({"roles":{"worker":configured,"editor":configured}})
}

async fn new_core(store: Store, config: Value) -> Core {
    Core::with_model_config_and_resolver(store, config, Arc::new(|_| Ok("fixture-only-key".into())))
        .unwrap()
}

async fn start_bridge_http(bridge: Arc<DshBridge>) -> (String, tokio::task::JoinHandle<()>) {
    let actor_resolver: ActorResolver = Arc::new(|headers: &HeaderMap| {
        (headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer local-harness"))
        .then(|| "integration-actor".to_owned())
        .ok_or_else(|| "unauthorized".to_owned())
    });
    let app = Router::new().nest("/v1", bridge_router(bridge, actor_resolver));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/v1/chat/completions"), task)
}

async fn new_bridge(store: Store, core: Core) -> Arc<DshBridge> {
    Arc::new(
        DshBridge::new(store, Arc::new(CoreRuntime::new(core)), bridge_config())
            .await
            .unwrap(),
    )
}

fn harness_tool_table() -> Value {
    json!([{"type":"function","function":{"name":"calculator","description":"Add two integers","parameters":{
        "type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"],"additionalProperties":false
    }}}])
}

fn request(messages: Value) -> Value {
    json!({
        "model":"sic-worker",
        "messages":messages,
        "stream":true,
        "stream_options":{"include_usage":true},
        "tools":harness_tool_table(),
        "tool_choice":"auto",
        "max_tokens":4096,
        "reasoning_effort":"high"
    })
}

fn session_headers(session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer local-harness"),
    );
    headers.insert(
        "x-sic-bridge-session",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

async fn post_chat(endpoint: &str, session: &str, body: &Value) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(endpoint)
        .headers(session_headers(session))
        .json(body)
        .send()
        .await
        .unwrap()
}

async fn reopen_store(path: &Path) -> Store {
    // Close the only old pool before reopening the same on-disk Core database.
    Store::open(path).await.unwrap()
}

#[tokio::test]
async fn real_core_bridge_runs_editor_worker_host_tool_and_recovers_without_repeating_calls() {
    let provider = ProviderFixture::default();
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_address = provider_listener.local_addr().unwrap();
    let provider_for_server = provider.clone();
    let provider_server = tokio::spawn(async move {
        axum::serve(
            provider_listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(provider_for_server),
        )
        .await
        .unwrap();
    });
    let _provider_state = provider;
    let model_config = provider_config(format!("http://{provider_address}/v1"));
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("dsh-core-integration.db");
    let store = Store::create(&database).await.unwrap();
    let core = new_core(store.clone(), model_config.clone()).await;
    let bridge = new_bridge(store.clone(), core.clone()).await;
    let (endpoint, bridge_server) = start_bridge_http(bridge.clone()).await;
    let session = "core-integration-session";
    let owner = sic_core_rust::core::OperationContext {
        actor: "integration-actor".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let created = core
        .operate("chain.create", &owner, &json!({}))
        .await
        .unwrap();
    let existing_chain = created["result"]["chain_id"].as_str().unwrap().to_owned();
    let bind_url = endpoint.replace("/chat/completions", "/sic/bind");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let payload = json!({"session_id":session,"chain_id":existing_chain});
    assert_eq!(
        client
            .post(&bind_url)
            .json(&payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let bound = client
        .post(&bind_url)
        .header("authorization", "Bearer local-harness")
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(bound.status(), StatusCode::OK);
    assert_eq!(bound.json::<Value>().await.unwrap()["replayed"], false);
    assert!(
        bridge
            .bind_existing_session("other-actor", session, &existing_chain)
            .await
            .is_err()
    );
    assert_eq!(
        bridge
            .bind_existing_session("integration-actor", session, &existing_chain)
            .await
            .unwrap()["replayed"],
        true
    );
    assert!(
        bridge
            .bind_existing_session("integration-actor", "another-session", &existing_chain)
            .await
            .is_err()
    );
    let initial = request(json!([
        {"role":"system","content":"Use only the provided tools and preserve source facts."},
        {"role":"user","content":"Add 7 and 5 with the calculator."}
    ]));

    let mut mismatched_effort = initial.clone();
    mismatched_effort["reasoning_effort"] = json!("low");
    let rejected = post_chat(&endpoint, "bad-effort", &mismatched_effort).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(_provider_state.requests.lock().await.len(), 0);

    let issued = post_chat(&endpoint, session, &initial).await;
    let issued_status = issued.status();
    let issued_sse = issued.text().await.unwrap();
    assert_eq!(issued_status, StatusCode::OK, "{issued_sse}");
    assert!(issued_sse.contains("worker-host-1"), "{issued_sse}");
    assert!(issued_sse.contains("calculator"), "{issued_sse}");
    assert!(issued_sse.contains("worker-host-1"), "{issued_sse}");
    let task_context = sic_core_rust::core::OperationContext {
        actor: "integration-actor".into(),
        chain_id: Some(existing_chain.clone()),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let claimed_tasks = core
        .operate("tools.tasks", &task_context, &json!({}))
        .await
        .unwrap();
    let issued_task = claimed_tasks["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["call_id"] == "worker-host-1")
        .unwrap();
    assert_eq!(issued_task["status"], "claimed");
    assert!(
        issued_task["claim_id"]
            .as_str()
            .unwrap()
            .starts_with("dsh:")
    );

    // Persisted issued calls must survive a bridge/Core restart without being reissued.
    bridge_server.abort();
    let _ = bridge_server.await;
    drop(bridge);
    drop(core);
    drop(store);
    let reopened = reopen_store(&database).await;
    let recovered_core = new_core(reopened.clone(), model_config.clone()).await;
    let recovered_bridge = new_bridge(reopened.clone(), recovered_core.clone()).await;
    let (endpoint, bridge_server) = start_bridge_http(recovered_bridge.clone()).await;
    let duplicate_issue = post_chat(&endpoint, session, &initial).await;
    assert_eq!(duplicate_issue.status(), StatusCode::CONFLICT);
    let duplicate_body = duplicate_issue.text().await.unwrap();
    assert!(
        duplicate_body.contains("turn_incomplete"),
        "{duplicate_body}"
    );
    let recovered_tasks = recovered_core
        .operate("tools.tasks", &task_context, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        recovered_tasks["result"]["tasks"][0]["status"], "claimed",
        "restart must preserve the claim without reissuing the DSH call"
    );

    let mut result_request = request(json!([
        {"role":"system","content":"Use only the provided tools and preserve source facts."},
        {"role":"user","content":"Add 7 and 5 with the calculator."},
        {"role":"assistant","content":null,"tool_calls":[{
            "id":"worker-host-1","type":"function","function":{"name":"calculator","arguments":"{\"a\":7,\"b\":5}"}
        }]},
        {"role":"tool","tool_call_id":"worker-host-1","name":"calculator","content":"12"}
    ]));
    // The host may remove its native catalog while delivering an outstanding
    // result. Completion must occur before ACL/schema synchronization.
    result_request["tools"] = json!([]);
    let completed = post_chat(&endpoint, session, &result_request).await;
    let completed_status = completed.status();
    let completed_sse = completed.text().await.unwrap();
    assert_eq!(completed_status, StatusCode::OK, "{completed_sse}");
    assert!(
        completed_sse.contains("Calculator returned 12; integration complete."),
        "{completed_sse}"
    );

    // A delivered result receipt remains replayable after a second restart.
    bridge_server.abort();
    let _ = bridge_server.await;
    drop(recovered_bridge);
    drop(recovered_core);
    drop(reopened);
    let reopened = reopen_store(&database).await;
    let restarted_core = new_core(reopened.clone(), model_config).await;
    let restarted_bridge = new_bridge(reopened.clone(), restarted_core.clone()).await;
    let (endpoint, bridge_server) = start_bridge_http(restarted_bridge.clone()).await;
    let replay = post_chat(&endpoint, session, &result_request).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.text().await.unwrap(), completed_sse);

    let calls = _provider_state.requests.lock().await;
    assert_eq!(
        calls.len(),
        5,
        "provider calls must not repeat across recovery"
    );
    assert_eq!(calls[0]["reasoning_effort"], "high");
    assert_eq!(calls[4]["reasoning_effort"], "high");
    assert_eq!(calls[0]["tools"][0]["function"]["name"], "sic_edit_blocks");
    assert!(
        calls[1].to_string().contains("integration CB fact"),
        "Worker should receive the CB published by the Editor"
    );
    assert!(
        calls[1]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| { tool["function"]["name"] == "sic_tool_search" })
    );
    assert!(
        calls[3]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "tool"
                    && message["tool_call_id"] == "worker-expand-1"
                    && message["content"]
                        .as_str()
                        .unwrap_or("")
                        .contains("calculator")
            })
    );

    let bridge_status = restarted_bridge
        .session_status("integration-actor", session)
        .await
        .unwrap();
    let chain_id = bridge_status["chain_id"].as_str().unwrap();
    assert_eq!(chain_id, existing_chain);
    let context = sic_core_rust::core::OperationContext {
        actor: "integration-actor".into(),
        chain_id: Some(chain_id.into()),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    let chain_status = restarted_core
        .operate("chain.get", &context, &json!({}))
        .await;
    let chain_status = chain_status.unwrap();
    assert!(chain_status["result"]["cut_seq"].as_u64().unwrap_or(0) > 0);
    let tool_status = restarted_core
        .operate("tools.status", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(tool_status["result"]["active_leases"], json!([]));
    assert_eq!(tool_status["result"]["catalog"], json!([]));
    bridge_server.abort();
    provider_server.abort();
}

#[tokio::test]
async fn dsh_binding_refuses_a_chain_with_pending_work() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let mut context = sic_core_rust::core::OperationContext {
        actor: "integration-actor".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let created = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    let chain = created["result"]["chain_id"].as_str().unwrap();
    context.chain_id = Some(chain.into());
    core.operate(
        "source.ingest",
        &context,
        &json!({"request":{"messages":[{"role":"user","content":"unprocessed"}]}}),
    )
    .await
    .unwrap();
    let bridge = new_bridge(store, core).await;
    let error = bridge
        .bind_existing_session("integration-actor", "pending-session", chain)
        .await
        .unwrap_err();
    assert_eq!(error.code, "chain_not_idle");
    assert!(
        bridge
            .session_status("integration-actor", "pending-session")
            .await
            .is_err()
    );
}
