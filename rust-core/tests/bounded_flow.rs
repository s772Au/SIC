pub mod core {
    pub use sic_core_rust::core::*;
}
pub mod error {
    pub use sic_core_rust::error::*;
}
pub mod storage {
    pub use sic_core_rust::storage::*;
}
#[path = "../src/scheduler.rs"]
mod scheduler;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use scheduler::{EditorMode, SchedulerPolicy, schedule};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone, Default)]
struct MockProvider {
    calls: Arc<Mutex<Vec<Value>>>,
    first_call: Arc<Notify>,
    gate_first: Option<Arc<Semaphore>>,
    fail: bool,
}

async fn completion(
    State(mock): State<MockProvider>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let call_number = {
        let mut calls = mock.calls.lock().await;
        calls.push(body);
        calls.len()
    };
    if call_number == 1 {
        mock.first_call.notify_one();
        if let Some(gate) = &mock.gate_first {
            gate.acquire().await.unwrap().forget();
        }
    }
    if mock.fail {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":{"message":"fixture provider outcome unknown"}})),
        );
    }
    (
        StatusCode::OK,
        Json(
            json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":16,"completion_tokens":8},
            "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                "tool_calls":[{"id":format!("edit-{call_number}"),"type":"function","function":{
                    "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}}]}),
        ),
    )
}

fn model_profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"SIC_TEST_MOCK_KEY",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native",
        "context_window":1000000,"max_input_tokens":1000000,"max_output_tokens":65536,
        "default_output_tokens":4096,"timeout_seconds":10})
}

async fn fixture(mock: MockProvider) -> (Core, OperationContext, tokio::task::JoinHandle<()>) {
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
    let profile = model_profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "scheduler-owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let created = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(created["result"]["chain_id"].as_str().unwrap().into());
    core.operate(
        "config.update",
        &context,
        &json!({"editor_feedback":{"max_corrections":0,"feedback_chars":8192}}),
    )
    .await
    .unwrap();
    (core, context, server)
}

fn policy(max_level: u32) -> SchedulerPolicy {
    let mut levels = std::collections::BTreeMap::new();
    if max_level >= 2 {
        levels.insert(2, 2);
    }
    if max_level >= 3 {
        levels.insert(3, 2);
    }
    SchedulerPolicy {
        context_id: "main".into(),
        min_sources_per_wb: 1,
        max_sources_per_wb: 1,
        max_units_per_wb: 64,
        level_batch_sizes: levels,
        max_level,
        flush: false,
        max_actions: 256,
        max_model_calls: 16,
        editor_mode: EditorMode::Provider,
        program_rules: vec!["deduplicate_exact".into()],
    }
}

async fn ingest(core: &Core, context: &OperationContext, label: &str) {
    core.operate(
        "source.ingest",
        context,
        &json!({"request":{"messages":[{"role":"user","content":label}]}}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn host_operation_exposes_the_bounded_scheduler() {
    let (core, context, server) = fixture(MockProvider::default()).await;
    ingest(&core, &context, "operation-entry").await;
    let mut p = policy(1);
    p.editor_mode = EditorMode::ProgramFixture;
    let result = core
        .operate(
            "runtime.schedule",
            &context,
            &serde_json::to_value(p).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result["result"]["published"].as_array().unwrap().len(), 1);
    assert_eq!(result["evidence"]["provider_called"], false);
    server.abort();
}

#[tokio::test]
async fn provider_edits_build_three_levels_and_worker_waits_while_editor_is_inflight() {
    let gate = Arc::new(Semaphore::new(0));
    let mock = MockProvider {
        gate_first: Some(gate.clone()),
        ..Default::default()
    };
    let calls = mock.calls.clone();
    let (core, context, server) = fixture(mock.clone()).await;
    for n in 0..4 {
        ingest(&core, &context, &format!("source-{n}")).await;
    }

    let scheduler_core = core.clone();
    let scheduler_context = context.clone();
    let scheduled =
        tokio::spawn(
            async move { schedule(&scheduler_core, &scheduler_context, &policy(3)).await },
        );
    tokio::time::timeout(Duration::from_secs(5), mock.first_call.notified())
        .await
        .unwrap();
    let worker_wait = core
        .operate("worker.run", &context, &json!({}))
        .await
        .unwrap_err();
    assert_eq!(worker_wait.code, "editor_pending");
    gate.add_permits(1);

    let report = tokio::time::timeout(Duration::from_secs(20), scheduled)
        .await
        .unwrap()
        .unwrap();
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert_eq!(report.published.len(), 7);
    assert!(report.published.iter().all(|block| block.provider_called));
    assert!(
        report
            .published
            .iter()
            .any(|block| block.level == 3 && block.provider_called)
    );
    assert_eq!(calls.lock().await.len(), 7);

    let third = core
        .operate(
            "hierarchy.timeline",
            &context,
            &json!({"context_id":"main","level":3,"kind":"cb"}),
        )
        .await
        .unwrap();
    assert_eq!(third["result"]["total"], 1);
    assert_eq!(
        third["result"]["rows"][0]["members"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    server.abort();
}

#[tokio::test]
async fn small_batch_waits_until_explicit_flush_then_publishes() {
    let mock = MockProvider::default();
    let (core, context, server) = fixture(mock).await;
    ingest(&core, &context, "small pending source").await;
    let mut p = policy(1);
    p.min_sources_per_wb = 2;
    p.max_sources_per_wb = 4;
    p.max_model_calls = 1;

    let waiting = schedule(&core, &context, &p).await;
    assert!(waiting.published.is_empty());
    assert!(waiting.needs_flush);
    assert!(waiting.wait_reason.as_deref().unwrap().contains("flush"));

    p.flush = true;
    let flushed = schedule(&core, &context, &p).await;
    assert!(flushed.failure.is_none(), "{:?}", flushed.failure);
    assert_eq!(flushed.published.len(), 1);
    assert_eq!(flushed.published[0].level, 1);
    assert!(flushed.published[0].provider_called);
    server.abort();
}

#[tokio::test]
async fn provider_error_halts_without_scheduler_retry() {
    let mock = MockProvider {
        fail: true,
        ..Default::default()
    };
    let calls = mock.calls.clone();
    let (core, context, server) = fixture(mock).await;
    ingest(&core, &context, "unknown provider outcome").await;
    let mut p = policy(1);
    p.max_sources_per_wb = 1;
    p.max_model_calls = 1;
    let report = schedule(&core, &context, &p).await;
    assert!(report.failure.is_some());
    assert!(report.published.is_empty());
    assert_eq!(report.model_calls_reserved_unknown, 1);
    assert_eq!(calls.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn program_fixture_is_explicit_and_never_reported_as_provider_execution() {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let (core, context, server) = fixture(mock).await;
    ingest(&core, &context, "local program fixture").await;
    let mut p = policy(1);
    p.editor_mode = EditorMode::ProgramFixture;
    p.max_model_calls = 0;
    let report = schedule(&core, &context, &p).await;
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert_eq!(report.editor_mode, EditorMode::ProgramFixture);
    assert_eq!(report.model_calls_observed, 0);
    assert_eq!(report.published.len(), 1);
    assert!(!report.published[0].provider_called);
    assert!(calls.lock().await.is_empty());
    server.abort();
}

#[tokio::test]
async fn action_and_model_call_budgets_are_hard_caps() {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let (core, context, server) = fixture(mock).await;
    ingest(&core, &context, "budget source").await;

    let mut action_limited = policy(1);
    action_limited.max_actions = 1;
    let action_report = schedule(&core, &context, &action_limited).await;
    assert_eq!(
        action_report.failure.as_ref().unwrap().code,
        "action_budget"
    );
    assert_eq!(action_report.actions, 1);
    assert!(calls.lock().await.is_empty());

    let mut model_limited = policy(1);
    model_limited.max_model_calls = 0;
    let model_report = schedule(&core, &context, &model_limited).await;
    assert_eq!(
        model_report.failure.as_ref().unwrap().code,
        "model_call_budget"
    );
    assert_eq!(model_report.model_calls_observed, 0);
    assert!(calls.lock().await.is_empty());
    server.abort();
}
