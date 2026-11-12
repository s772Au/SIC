use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone)]
struct MockProvider {
    calls: Arc<Mutex<Vec<Value>>>,
    failure_status: StatusCode,
    failures: usize,
}

async fn completion(
    State(mock): State<MockProvider>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let number = {
        let mut calls = mock.calls.lock().await;
        calls.push(body);
        calls.len()
    };
    if number <= mock.failures {
        if mock.failure_status == StatusCode::REQUEST_TIMEOUT {
            // Simulate a connection that accepts the request but does not
            // return any HTTP status before the host deadline.
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        return (
            mock.failure_status,
            Json(json!({"error":{"message":"synthetic failure"}})),
        );
    }
    (
        StatusCode::OK,
        Json(
            json!({"model":"fixed-model","usage":{"prompt_tokens":7,"completion_tokens":3},
        "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"edit-fixture","type":"function","function":{
            "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}}]}),
        ),
    )
}

async fn fixture(
    status: StatusCode,
    failures: usize,
    enabled: bool,
    corrections: u64,
) -> (
    Core,
    OperationContext,
    Value,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mock = MockProvider {
        calls: calls.clone(),
        failure_status: status,
        failures,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(mock),
        )
        .await
        .unwrap();
    });
    let profile = json!({"model":"openai/fixed-model","api_base":format!("http://{address}/v1"),
        "api_key_env":"TEST_KEY","tokenizer_model":"host-o200k-surrogate","context_window":1000000,
        "max_input_tokens":1000000,"max_output_tokens":4096,"default_output_tokens":1024,
        "timeout_seconds":if status == StatusCode::REQUEST_TIMEOUT { 1 } else { 5 },"retry_editor_http_5xx":enabled});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(chain["result"]["chain_id"].as_str().unwrap().into());
    core.operate(
        "config.update",
        &context,
        &json!({"editor_feedback":{"max_corrections":corrections,"feedback_chars":8192}}),
    )
    .await
    .unwrap();
    let source = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[{"role":"user","content":"synthetic retry"}]}}),
        )
        .await
        .unwrap();
    (
        core,
        context,
        source["result"]["source_id"].clone(),
        calls,
        server,
    )
}

async fn prepare(core: &Core, context: &OperationContext, source: Value) -> Value {
    core.operate("edit.prepare", context, &json!({"source_id":source}))
        .await
        .unwrap()["result"]["job_id"]
        .clone()
}

#[tokio::test]
async fn configured_5xx_retry_reuses_request_and_preserves_unknown_usage() {
    let (core, context, source, calls, server) =
        fixture(StatusCode::INTERNAL_SERVER_ERROR, 1, true, 1).await;
    let job = prepare(&core, &context, source).await;
    let result = core
        .operate("edit.run", &context, &json!({"job_id":job}))
        .await
        .unwrap();
    assert_eq!(result["result"]["status"], "ready");
    assert_eq!(result["result"]["attempts"], 2);
    let sent = calls.lock().await;
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[0], sent[1],
        "a transport retry must not inject model feedback"
    );
    drop(sent);
    let ledger = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    let groups = ledger["result"]["groups"].as_array().unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|g| g["attempts"].as_u64().unwrap())
            .sum::<u64>(),
        2
    );
    assert_eq!(
        groups
            .iter()
            .map(|g| g["prompt_tokens"]["known_tokens"].as_u64().unwrap())
            .sum::<u64>(),
        7
    );
    assert_eq!(
        groups
            .iter()
            .map(|g| g["prompt_tokens"]["unknown_attempts"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    assert_eq!(ledger["result"]["complete"], false);
    assert_eq!(ledger["result"]["unresolved_attempts"], 0);
    server.abort();
}

#[tokio::test]
async fn retry_is_opt_in_and_never_replays_rate_limit_or_exceeds_budget() {
    for (status, enabled, corrections, expected) in [
        (StatusCode::INTERNAL_SERVER_ERROR, false, 1, 1),
        (StatusCode::TOO_MANY_REQUESTS, true, 1, 1),
        (StatusCode::REQUEST_TIMEOUT, true, 1, 1),
        (StatusCode::UNAUTHORIZED, true, 1, 1),
        (StatusCode::INTERNAL_SERVER_ERROR, true, 0, 1),
        (StatusCode::BAD_GATEWAY, true, 1, 2),
    ] {
        let (core, context, source, calls, server) =
            fixture(status, 10, enabled, corrections).await;
        let job = prepare(&core, &context, source).await;
        assert!(
            core.operate("edit.run", &context, &json!({"job_id":job}))
                .await
                .is_err()
        );
        assert_eq!(calls.lock().await.len(), expected);
        server.abort();
    }
}

#[tokio::test]
async fn host_abandon_during_backoff_prevents_another_provider_call() {
    let (core, context, source, calls, server) =
        fixture(StatusCode::SERVICE_UNAVAILABLE, 1, true, 1).await;
    let job = prepare(&core, &context, source).await;
    let running_core = core.clone();
    let running_context = context.clone();
    let running_job = job.clone();
    let running = tokio::spawn(async move {
        running_core
            .operate("edit.run", &running_context, &json!({"job_id":running_job}))
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ledger = core
            .operate("accounting.get", &context, &json!({}))
            .await
            .unwrap();
        if ledger["result"]["attempts"].as_array().unwrap().len() == 1
            && ledger["result"]["unresolved_attempts"] == 0
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "first failed attempt was not recorded"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let stopped = core
        .operate(
            "runtime.abandon",
            &context,
            &json!({"kind":"editor","job_id":job}),
        )
        .await
        .unwrap();
    assert_eq!(stopped["result"]["abandoned"], true);
    assert!(running.await.unwrap().is_err());
    assert_eq!(calls.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn scheduler_reserves_all_retry_attempts_before_dispatch() {
    let (core, context, _source, calls, server) =
        fixture(StatusCode::INTERNAL_SERVER_ERROR, 1, true, 1).await;
    let policy = |max| {
        json!({"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":1,
        "max_units_per_wb":64,"level_batch_sizes":{},"max_level":1,"flush":true,"max_actions":32,
        "max_model_calls":max,"editor_mode":"provider","program_rules":[]})
    };
    let refused = core
        .operate("runtime.schedule", &context, &policy(1))
        .await
        .unwrap();
    assert_eq!(refused["result"]["completed"], false);
    assert_eq!(calls.lock().await.len(), 0);
    let accepted = core
        .operate("runtime.schedule", &context, &policy(2))
        .await
        .unwrap();
    assert_eq!(accepted["result"]["completed"], true);
    assert_eq!(accepted["result"]["model_calls_observed"], 2);
    assert_eq!(calls.lock().await.len(), 2);
    server.abort();
}
