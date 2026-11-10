use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use sic_core_rust::{
    core::Core,
    dsh_bridge::{ActorResolver, CoreRuntime, DshBridge, DshBridgeConfig, router as bridge_router},
    storage::Store,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn completion(
    State(count): State<Arc<AtomicUsize>>,
    Json(_body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let n = count.fetch_add(1, Ordering::SeqCst);
    if n == 1 {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"synthetic failure"})),
        );
    }
    let (finish, message) = if n == 0 || n == 2 {
        (
            "tool_calls",
            json!({"role":"assistant","content":null,
        "tool_calls":[{"id":format!("edit-{n}"),"type":"function","function":{
            "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}),
        )
    } else {
        (
            "stop",
            json!({"role":"assistant","content":"synthetic recovery completed"}),
        )
    };
    (
        StatusCode::OK,
        Json(json!({"model":"mock-model","choices":[{"finish_reason":finish,"message":message}]})),
    )
}

async fn tool_then_failure(
    State(count): State<Arc<AtomicUsize>>,
    Json(_body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let n = count.fetch_add(1, Ordering::SeqCst);
    if n >= 2 {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"synthetic failure"})),
        );
    }
    let (name, args) = if n == 0 {
        ("sic_edit_blocks", json!({"ops":[],"pvi":{}}))
    } else {
        ("sic_tool_call", json!({"name":"fixture","arguments":{}}))
    };
    (
        StatusCode::OK,
        Json(
            json!({"model":"mock-model","choices":[{"finish_reason":"tool_calls",
        "message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("call-{n}"),"type":"function",
        "function":{"name":name,"arguments":args.to_string()}}]}}]}),
        ),
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
    json!({"model":"sic-worker","messages":[{"role":"user","content":text}],
    "stream":true,"tools":[],"tool_choice":"auto","max_tokens":4096})
}

#[tokio::test]
async fn failed_turn_can_be_retired_then_idle_agent_bound_to_a_fresh_session() {
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{address}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"fixture","context_window":1000000,
        "max_input_tokens":1000000,"max_output_tokens":393216,"default_output_tokens":4096,"timeout_seconds":10});
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("local-fixture".into())),
    )
    .unwrap();
    let bridge = Arc::new(
        DshBridge::new(
            store,
            Arc::new(CoreRuntime::new(core)),
            DshBridgeConfig {
                model_alias: "sic-worker".into(),
                fixed_max_tokens: Some(4096),
                max_worker_calls_per_turn: 8,
                max_host_tasks_per_response: 4,
                max_harness_tools: 128,
                fixed_reasoning_effort: None,
            },
        )
        .await
        .unwrap(),
    );
    assert!(
        bridge
            .handle(
                "owner",
                &headers("old"),
                &request("Original synthetic task")
            )
            .await
            .is_err()
    );
    let status = bridge.session_status("owner", "old").await.unwrap();
    assert_eq!(status["state"], "indeterminate");
    assert_eq!(status["phase"], "worker_model_started");
    let fingerprint = status["active_fingerprint"].as_str().unwrap();
    let chain = status["chain_id"].as_str().unwrap();
    assert!(
        bridge
            .retire_unresolved_session("other", "old", fingerprint, "operator inspected failure")
            .await
            .is_err()
    );
    assert!(
        bridge
            .retire_unresolved_session(
                "owner",
                "old",
                &"f".repeat(64),
                "operator inspected failure"
            )
            .await
            .is_err()
    );
    let resolver: ActorResolver = Arc::new(|headers: &HeaderMap| {
        (headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer fixture-control"))
        .then(|| "owner".into())
        .ok_or_else(|| "denied".into())
    });
    let control_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let control_address = control_listener.local_addr().unwrap();
    let control_server = tokio::spawn(
        axum::serve(
            control_listener,
            Router::new().nest("/v1", bridge_router(bridge.clone(), resolver)),
        )
        .into_future(),
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let endpoint = format!("http://{control_address}/v1/sic/retire");
    let body =
        json!({"session_id":"old","fingerprint":fingerprint,"reason":"operator inspected failure"});
    assert_eq!(
        client
            .post(&endpoint)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let result = client
        .post(&endpoint)
        .bearer_auth("fixture-control")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);
    let retired: Value = result.json().await.unwrap();
    assert_eq!(retired["replayed"], false);
    assert_eq!(retired["external_work_replayed"], false);
    assert_eq!(
        bridge
            .retire_unresolved_session("owner", "old", fingerprint, "operator inspected failure")
            .await
            .unwrap()["replayed"],
        true
    );
    assert!(
        bridge
            .retire_unresolved_session("owner", "old", fingerprint, "different reason")
            .await
            .is_err()
    );
    assert_eq!(
        bridge
            .handle(
                "owner",
                &headers("old"),
                &request("Original synthetic task")
            )
            .await
            .unwrap_err()
            .code,
        "session_retired"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    bridge
        .bind_existing_session("owner", "new", chain)
        .await
        .unwrap();
    let response = bridge
        .handle(
            "owner",
            &headers("new"),
            &request("Continue the synthetic task"),
        )
        .await
        .unwrap();
    assert!(
        String::from_utf8(response)
            .unwrap()
            .contains("synthetic recovery completed")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        bridge.session_status("owner", "old").await.unwrap()["retired"],
        true
    );
    control_server.abort();
    server.abort();
}

#[tokio::test]
async fn retirement_does_not_release_a_chain_with_unresolved_tool_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(tool_then_failure))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{address}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"fixture","context_window":1000000,
        "max_input_tokens":1000000,"max_output_tokens":393216,"default_output_tokens":4096,"timeout_seconds":10});
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("local-fixture".into())),
    )
    .unwrap();
    let bridge = DshBridge::new(
        store,
        Arc::new(CoreRuntime::new(core)),
        DshBridgeConfig {
            model_alias: "sic-worker".into(),
            fixed_max_tokens: Some(4096),
            max_worker_calls_per_turn: 8,
            max_host_tasks_per_response: 4,
            max_harness_tools: 128,
            fixed_reasoning_effort: None,
        },
    )
    .await
    .unwrap();
    let tools = json!([{"type":"function","function":{"name":"fixture","description":"local fixture",
        "parameters":{"type":"object","properties":{},"additionalProperties":false}}}]);
    let mut first = request("Use the local fixture");
    first["tools"] = tools.clone();
    let issued = bridge
        .handle("owner", &headers("tool-old"), &first)
        .await
        .unwrap();
    assert!(String::from_utf8(issued).unwrap().contains("fixture"));
    let mut result = request("unused");
    result["tools"] = tools;
    result["messages"] = json!([{"role":"user","content":"Use the local fixture"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function",
        "function":{"name":"fixture","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call-1","name":"fixture","content":"fixture result"}]);
    assert!(
        bridge
            .handle("owner", &headers("tool-old"), &result)
            .await
            .is_err()
    );
    let status = bridge.session_status("owner", "tool-old").await.unwrap();
    let fingerprint = status["active_fingerprint"].as_str().unwrap();
    let chain = status["chain_id"].as_str().unwrap();
    bridge
        .retire_unresolved_session(
            "owner",
            "tool-old",
            fingerprint,
            "operator inspected tool failure",
        )
        .await
        .unwrap();
    let error = bridge
        .bind_existing_session("owner", "tool-new", chain)
        .await
        .unwrap_err();
    assert_eq!(error.code, "chain_not_idle");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.abort();
}
