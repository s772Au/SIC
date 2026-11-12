mod model_policy {
    pub use sic_core_rust::model_policy::*;
}

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
use dsh_bridge::{
    ActorResolver, CoreRuntime, DshBridge, DshBridgeConfig, DshRuntime, router as bridge_router,
};
use serde_json::{Value, json};
use sic_core_rust::{core::Core, storage::Store};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
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

#[derive(Clone)]
struct InvalidChoiceFixture {
    requests: Arc<Mutex<Vec<Value>>>,
    mode: &'static str,
}

async fn invalid_choice_completion(
    State(fixture): State<InvalidChoiceFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request);
        requests.len()
    };
    if index == 1 {
        return (
            StatusCode::OK,
            Json(json!({"model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{"finish_reason":"tool_calls",
            "message":{"role":"assistant","content":null,"tool_calls":[{"id":"editor-one","type":"function",
            "function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}}]})),
        );
    }
    if fixture.mode == "http_failure" {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{"message":"synthetic"}})),
        );
    }
    if index == 2 || fixture.mode == "always_invalid" {
        return (
            StatusCode::OK,
            Json(json!({"model":"deepseek-v4.1-flash-eco",
                "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[
                {"index":0,"message":{"role":"assistant","content":"synthetic ambiguous candidate"}},
                {"index":1,"finish_reason":"tool_calls","message":{"role":"assistant","tool_calls":[{
                    "id":"ambiguous-call","type":"function","function":{"name":"sic_tool_call",
                    "arguments":json!({"name":"calculator","arguments":{"a":1,"b":2}}).to_string()}}]}}
            ]})),
        );
    }
    let (finish, message) = worker_tool_call(
        "actual-call",
        "sic_tool_call",
        json!({"name":"calculator","arguments":{"a":7,"b":5}}),
    );
    (
        StatusCode::OK,
        Json(json!({"model":"deepseek-v4.1-flash-eco",
        "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{"finish_reason":finish,"message":message}]})),
    )
}

#[tokio::test]
async fn dsh_retries_only_rejected_complete_choice_sets_without_dispatching_them() {
    for mode in ["recover", "always_invalid", "http_failure"] {
        let fixture = InvalidChoiceFixture {
            requests: Arc::new(Mutex::new(Vec::new())),
            mode,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(invalid_choice_completion))
                    .with_state(fixture.clone()),
            )
            .into_future(),
        );
        let store = Store::memory().await.unwrap();
        let core = new_core(store.clone(), provider_config(format!("http://{addr}/v1"))).await;
        let bridge = DshBridge::new(
            store,
            Arc::new(CoreRuntime::new(core.clone())),
            bridge_config(),
        )
        .await
        .unwrap()
        .with_invalid_worker_choice_retries(1)
        .unwrap();
        let headers = session_headers("choice-retry");
        let body = request(json!([{"role":"user","content":"Use the calculator."}]));
        let result = bridge.handle("integration-actor", &headers, &body).await;
        let requests = fixture.requests.lock().await;
        if mode == "recover" {
            let text = String::from_utf8(result.unwrap()).unwrap();
            assert!(text.contains("actual-call"));
            assert!(!text.contains("ambiguous-call"));
            assert_eq!(requests.len(), 3);
            let status = bridge
                .session_status("integration-actor", "choice-retry")
                .await
                .unwrap();
            let context = sic_core_rust::core::OperationContext {
                actor: "integration-actor".into(),
                chain_id: status["chain_id"].as_str().map(str::to_owned),
                allow_write: true,
                allow_model: true,
                allow_revoke: false,
            };
            let tasks = core
                .operate("tools.tasks", &context, &json!({}))
                .await
                .unwrap();
            assert_eq!(tasks["result"]["tasks"].as_array().unwrap().len(), 1);
            assert_eq!(tasks["result"]["tasks"][0]["call_id"], "actual-call");
            let usage = core
                .operate("accounting.get", &context, &json!({}))
                .await
                .unwrap();
            let worker = usage["result"]["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|group| group["role"] == "worker")
                .unwrap();
            assert_eq!(worker["attempts"], 2);
            assert_eq!(worker["prompt_tokens"]["known_tokens"], 22);
        } else if mode == "always_invalid" {
            let error = result.unwrap_err();
            assert_eq!(error.code, "worker_invalid_choice_count");
            assert!(!error.message.contains("synthetic ambiguous"));
            assert_eq!(requests.len(), 3);
        } else {
            assert_eq!(result.unwrap_err().code, "core_failed");
            assert_eq!(requests.len(), 2, "HTTP failures are not retried");
        }
        server.abort();
    }
}

#[derive(Clone)]
struct HttpFailureFixture {
    requests: Arc<Mutex<Vec<Value>>>,
    status: StatusCode,
    always_fail: bool,
    timeout_worker: bool,
    invalid_choice: bool,
}

async fn worker_http_failure_completion(
    State(fixture): State<HttpFailureFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request);
        requests.len()
    };
    if index == 1 {
        return (
            StatusCode::OK,
            Json(json!({"model":"deepseek-v4.1-flash-eco",
                "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{
                    "finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":"editor-one","type":"function","function":{
                        "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}}]})),
        );
    }
    if index == 2 || fixture.always_fail {
        if fixture.timeout_worker {
            tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
        }
        if fixture.invalid_choice {
            return (
                StatusCode::OK,
                Json(json!({"model":"deepseek-v4.1-flash-eco",
                "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[
                    {"message":{"role":"assistant","content":"ambiguous one"}},
                    {"message":{"role":"assistant","content":"ambiguous two"}}
                ]})),
            );
        }
        // A non-success body must never be admitted even if it resembles a
        // valid tool call. Only the HTTP status can guide the retry policy.
        return (
            fixture.status,
            Json(
                json!({"choices":[{"message":{"role":"assistant","tool_calls":[{
                "id":"must-not-dispatch","type":"function","function":{"name":"sic_tool_call",
                "arguments":"{\"name\":\"calculator\",\"arguments\":{}}"}}]}}]}),
            ),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{
                "finish_reason":"stop","message":{"role":"assistant","content":"recovered"}}]})),
    )
}

#[tokio::test]
async fn dsh_worker_explicit_5xx_retry_is_opt_in_bounded_and_records_unknown_usage() {
    for (status, enabled, always_fail, timeout_worker, max_calls, succeeds, expected_calls) in [
        (500, Some(true), false, false, 8, true, 3),
        (502, Some(true), false, false, 8, true, 3),
        (503, Some(true), false, false, 8, true, 3),
        (504, Some(true), false, false, 8, true, 3),
        (500, Some(false), false, false, 8, false, 2),
        (500, None, false, false, 8, false, 2),
        (429, Some(true), false, false, 8, false, 2),
        (400, Some(true), false, false, 8, false, 2),
        (501, Some(true), false, false, 8, false, 2),
        (500, Some(true), true, false, 8, false, 3),
        (500, Some(true), false, false, 1, false, 2),
        (500, Some(true), false, true, 8, false, 2),
    ] {
        let fixture = HttpFailureFixture {
            requests: Arc::new(Mutex::new(Vec::new())),
            status: StatusCode::from_u16(status).unwrap(),
            always_fail,
            timeout_worker,
            invalid_choice: false,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(worker_http_failure_completion))
                    .with_state(fixture.clone()),
            )
            .into_future(),
        );
        let store = Store::memory().await.unwrap();
        let mut model_config = provider_config(format!("http://{addr}/v1"));
        if let Some(enabled) = enabled {
            model_config["roles"]["worker"]["retry_worker_http_5xx"] = json!(enabled);
        }
        if timeout_worker {
            model_config["roles"]["worker"]["timeout_seconds"] = json!(1);
        }
        let core = new_core(store.clone(), model_config).await;
        let mut config = bridge_config();
        config.max_worker_calls_per_turn = max_calls;
        let bridge = DshBridge::new(store, Arc::new(CoreRuntime::new(core.clone())), config)
            .await
            .unwrap();
        let headers = session_headers("http-retry");
        let body = request(json!([{"role":"user","content":"synthetic task"}]));
        let result = bridge.handle("integration-actor", &headers, &body).await;
        if succeeds {
            let bytes = result.unwrap();
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(text.contains("recovered"));
            assert!(!text.contains("must-not-dispatch"));
            assert_eq!(
                bridge
                    .handle("integration-actor", &headers, &body)
                    .await
                    .unwrap(),
                bytes
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
            assert!(!error.message.contains("must-not-dispatch"));
        }
        let requests = fixture.requests.lock().await;
        assert_eq!(
            requests.len(),
            expected_calls,
            "HTTP {status}, enabled={enabled:?}, always_fail={always_fail}, timeout={timeout_worker}, budget={max_calls}"
        );
        if succeeds {
            assert_eq!(
                requests[1], requests[2],
                "retry must use the same frozen request"
            );
        }
        drop(requests);
        let session = bridge
            .session_status("integration-actor", "http-retry")
            .await
            .unwrap();
        let context = sic_core_rust::core::OperationContext {
            actor: "integration-actor".into(),
            chain_id: session["chain_id"].as_str().map(str::to_owned),
            allow_write: true,
            allow_model: true,
            allow_revoke: false,
        };
        let tasks = core
            .operate("tools.tasks", &context, &json!({}))
            .await
            .unwrap();
        assert!(tasks["result"]["tasks"].as_array().unwrap().is_empty());
        let usage = core
            .operate("accounting.get", &context, &json!({}))
            .await
            .unwrap();
        let worker_groups: Vec<&Value> = usage["result"]["groups"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|group| group["role"] == "worker")
            .collect();
        assert_eq!(
            worker_groups
                .iter()
                .map(|g| g["attempts"].as_u64().unwrap())
                .sum::<u64>(),
            (expected_calls - 1) as u64
        );
        assert_eq!(
            worker_groups
                .iter()
                .map(|g| g["prompt_tokens"]["unknown_attempts"].as_u64().unwrap())
                .sum::<u64>(),
            if always_fail { 2 } else { 1 }
        );
        server.abort();
    }
}

#[derive(Clone)]
struct SequentialWorkerFailureFixture {
    requests: Arc<Mutex<Vec<Value>>>,
    /// How many Worker calls (not counting the first Editor tool call) must fail.
    fail_workers: usize,
}

async fn sequential_worker_failure_completion(
    State(fixture): State<SequentialWorkerFailureFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request);
        requests.len()
    };
    if index == 1 {
        return (
            StatusCode::OK,
            Json(json!({"model":"deepseek-v4.1-flash-eco",
                "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{
                    "finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":"editor-one","type":"function","function":{
                        "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}}]})),
        );
    }
    if index - 1 <= fixture.fail_workers {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":{"message":"synthetic 503"}})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":11,"completion_tokens":7},"choices":[{
                "finish_reason":"stop","message":{"role":"assistant","content":"recovered"}}]})),
    )
}

#[tokio::test]
async fn worker_5xx_retry_attempts_follow_the_frozen_profile() {
    for (attempts, fail_workers, expected_calls, succeeds) in [
        (1u32, 1usize, 2usize, false),
        (2, 1, 3, true),
        (3, 2, 4, true),
        (3, 3, 4, false),
        (5, 9, 6, false),
    ] {
        let fixture = SequentialWorkerFailureFixture {
            requests: Arc::new(Mutex::new(Vec::new())),
            fail_workers,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/v1/chat/completions",
                        post(sequential_worker_failure_completion),
                    )
                    .with_state(fixture.clone()),
            )
            .into_future(),
        );
        let store = Store::memory().await.unwrap();
        let mut model_config = provider_config(format!("http://{addr}/v1"));
        model_config["roles"]["worker"]["retry_worker_http_5xx"] = json!(true);
        model_config["roles"]["worker"]["worker_retry_max_attempts"] = json!(attempts);
        model_config["roles"]["worker"]["worker_retry_backoff_ms"] = json!(0);
        let core = new_core(store.clone(), model_config).await;
        let mut config = bridge_config();
        config.max_worker_calls_per_turn = 8;
        let bridge = DshBridge::new(store, Arc::new(CoreRuntime::new(core)), config)
            .await
            .unwrap()
            .with_worker_http_retry_policy(attempts, 0)
            .unwrap();
        let headers = session_headers("http-retry-policy");
        let body = request(json!([{"role":"user","content":"synthetic task"}]));
        let result = bridge.handle("integration-actor", &headers, &body).await;
        let requests = fixture.requests.lock().await;
        assert_eq!(
            requests.len(),
            expected_calls,
            "attempts={attempts} fail_workers={fail_workers}"
        );
        if succeeds {
            let text = String::from_utf8(result.unwrap()).unwrap();
            assert!(text.contains("recovered"));
            assert_eq!(
                requests[1], requests[2],
                "every retry must reuse the same frozen Worker request"
            );
        } else {
            assert_eq!(result.unwrap_err().code, "worker_retryable_http_5xx");
        }
        drop(requests);
        server.abort();
    }
}

#[tokio::test]
async fn worker_5xx_retry_backoff_waits_between_attempts() {
    let fixture = SequentialWorkerFailureFixture {
        requests: Arc::new(Mutex::new(Vec::new())),
        fail_workers: 2,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(sequential_worker_failure_completion),
                )
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let store = Store::memory().await.unwrap();
    let mut model_config = provider_config(format!("http://{addr}/v1"));
    model_config["roles"]["worker"]["retry_worker_http_5xx"] = json!(true);
    model_config["roles"]["worker"]["worker_retry_max_attempts"] = json!(3);
    model_config["roles"]["worker"]["worker_retry_backoff_ms"] = json!(60);
    let core = new_core(store.clone(), model_config).await;
    let mut config = bridge_config();
    config.max_worker_calls_per_turn = 8;
    let bridge = DshBridge::new(store, Arc::new(CoreRuntime::new(core)), config)
        .await
        .unwrap()
        .with_worker_http_retry_policy(3, 60)
        .unwrap();
    let headers = session_headers("http-retry-backoff");
    let body = request(json!([{"role":"user","content":"synthetic task"}]));
    let started = std::time::Instant::now();
    let result = bridge.handle("integration-actor", &headers, &body).await;
    let elapsed = started.elapsed();
    assert!(
        String::from_utf8(result.unwrap())
            .unwrap()
            .contains("recovered")
    );
    assert_eq!(fixture.requests.lock().await.len(), 4);
    // First retry waits 60 ms, the second 120 ms; a lower bound is stable.
    assert!(
        elapsed >= std::time::Duration::from_millis(150),
        "two bounded retries must wait for their backoff, saw {elapsed:?}"
    );
    server.abort();
}

#[derive(Clone)]
struct MutatingRuntime {
    core: Core,
    inner: CoreRuntime,
    changed: Arc<AtomicBool>,
}

impl DshRuntime for MutatingRuntime {
    fn operate(
        &self,
        actor: String,
        chain_id: Option<String>,
        operation: String,
        payload: Value,
        allow_model: bool,
    ) -> dsh_bridge::BoxFuture<Result<Value, String>> {
        let this = self.clone();
        Box::pin(async move {
            let result = this
                .inner
                .operate(
                    actor.clone(),
                    chain_id.clone(),
                    operation.clone(),
                    payload,
                    allow_model,
                )
                .await;
            if operation == "worker.run"
                && result.as_ref().err().is_some_and(|error| {
                    error.starts_with("worker_retryable_http_5xx:")
                        || error.starts_with("worker_invalid_choice_count:")
                })
                && !this.changed.swap(true, Ordering::SeqCst)
            {
                let context = sic_core_rust::core::OperationContext {
                    actor,
                    chain_id,
                    allow_write: true,
                    allow_model: false,
                    allow_revoke: false,
                };
                this.core
                    .operate(
                        "source.ingest",
                        &context,
                        &json!({"request":{"messages":[{"role":"user","content":"new concurrent input"}]}}),
                    )
                    .await
                    .expect("isolated concurrent input must be accepted");
            }
            result
        })
    }
}

#[tokio::test]
async fn worker_retry_rejects_changed_context_before_another_provider_call() {
    for invalid_choice in [false, true] {
        let fixture = HttpFailureFixture {
            requests: Arc::new(Mutex::new(Vec::new())),
            status: StatusCode::SERVICE_UNAVAILABLE,
            always_fail: false,
            timeout_worker: false,
            invalid_choice,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(worker_http_failure_completion))
                    .with_state(fixture.clone()),
            )
            .into_future(),
        );
        let store = Store::memory().await.unwrap();
        let mut model_config = provider_config(format!("http://{addr}/v1"));
        model_config["roles"]["worker"]["retry_worker_http_5xx"] = json!(true);
        let core = new_core(store.clone(), model_config).await;
        let changed = Arc::new(AtomicBool::new(false));
        let runtime = MutatingRuntime {
            inner: CoreRuntime::new(core.clone()),
            core,
            changed: changed.clone(),
        };
        let bridge = DshBridge::new(store, Arc::new(runtime), bridge_config())
            .await
            .unwrap()
            .with_invalid_worker_choice_retries(1)
            .unwrap();
        let error = bridge
            .handle(
                "integration-actor",
                &session_headers("changed-retry"),
                &request(json!([{"role":"user","content":"original task"}])),
            )
            .await
            .unwrap_err();
        assert!(changed.load(Ordering::SeqCst));
        assert_eq!(error.code, "worker_retry_context_changed");
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(fixture.requests.lock().await.len(), 2);
        let accounting = bridge
            .session_accounting("integration-actor", "changed-retry")
            .await
            .unwrap();
        assert_eq!(
            accounting["groups"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|group| group["role"] == "worker")
                .map(|group| group["attempts"].as_u64().unwrap())
                .sum::<u64>(),
            1
        );
        server.abort();
    }
}

async fn tool_then_worker_http_failure(
    State(fixture): State<ProviderFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request);
        requests.len()
    };
    let (finish, message) = match index {
        1 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{
                "id":"editor","type":"function","function":{"name":"sic_edit_blocks",
                    "arguments":"{\"ops\":[],\"pvi\":{}}"}}]}),
        ),
        2 => worker_tool_call(
            "one-external-call",
            "sic_tool_call",
            json!({"name":"calculator","arguments":{"a":7,"b":5}}),
        ),
        3 => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"synthetic"})),
            );
        }
        4 => (
            "stop",
            json!({"role":"assistant","content":"result adopted"}),
        ),
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"unexpected"})),
            );
        }
    };
    (
        StatusCode::OK,
        Json(json!({"model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":11,"completion_tokens":7},
            "choices":[{"finish_reason":finish,"message":message}]})),
    )
}

#[tokio::test]
async fn worker_5xx_after_host_result_does_not_reissue_the_external_tool() {
    let fixture = ProviderFixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(tool_then_worker_http_failure))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let store = Store::memory().await.unwrap();
    let mut model_config = provider_config(format!("http://{addr}/v1"));
    model_config["roles"]["worker"]["retry_worker_http_5xx"] = json!(true);
    let core = new_core(store.clone(), model_config).await;
    let bridge = DshBridge::new(
        store,
        Arc::new(CoreRuntime::new(core.clone())),
        bridge_config(),
    )
    .await
    .unwrap();
    let headers = session_headers("retry-after-tool");
    let initial = request(json!([{"role":"user","content":"calculate"}]));
    let first = String::from_utf8(
        bridge
            .handle("integration-actor", &headers, &initial)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(first.contains("one-external-call"));
    let continuation = request(json!([
        {"role":"user","content":"calculate"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"one-external-call","type":"function",
            "function":{"name":"calculator","arguments":"{\"a\":7,\"b\":5}"}}]},
        {"role":"tool","tool_call_id":"one-external-call","name":"calculator","content":"12"}
    ]));
    let result = bridge
        .handle("integration-actor", &headers, &continuation)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&result).contains("result adopted"));
    assert_eq!(
        bridge
            .handle("integration-actor", &headers, &continuation)
            .await
            .unwrap(),
        result
    );
    let requests = fixture.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2], requests[3]);
    drop(requests);
    let session = bridge
        .session_status("integration-actor", "retry-after-tool")
        .await
        .unwrap();
    let context = sic_core_rust::core::OperationContext {
        actor: "integration-actor".into(),
        chain_id: session["chain_id"].as_str().map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let tasks = core
        .operate("tools.tasks", &context, &json!({}))
        .await
        .unwrap();
    let tasks = tasks["result"]["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["call_id"], "one-external-call");
    // The harness supplied a tool result, but no independent native event
    // attested its outcome. Preserve that distinction during retry.
    assert_eq!(tasks[0]["status"], "delivered_unattested");
    server.abort();
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
