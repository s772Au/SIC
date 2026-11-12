use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockState {
    calls: Arc<Mutex<Vec<Value>>>,
}

async fn completion(
    State(state): State<MockState>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    state.calls.lock().await.push(body);
    (
        StatusCode::OK,
        Json(json!({
            "model": "deepseek-v4.1-flash-eco",
            "usage": {"prompt_tokens": 11, "completion_tokens": 7},
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "edit-empty",
                        "type": "function",
                        "function": {"name": "sic_edit_blocks", "arguments": "{\"ops\":[],\"pvi\":{}}"}
                    }]
                }
            }]
        })),
    )
}

async fn fixture() -> (
    Core,
    OperationContext,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let state = MockState::default();
    let calls = state.calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(state),
        )
        .await;
    });
    let profile = json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "api_base": format!("http://{address}/v1"),
        "api_key_env": "SIC_TEST_MOCK_KEY",
        "tokenizer_model": "host-o200k-surrogate",
        "tokenizer_status": "surrogate-not-provider-native",
        "context_window": 1_000_000,
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 65536,
        "default_output_tokens": 4096,
        "timeout_seconds": 10
    });
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "owner".into(),
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
    (core, context, calls, server)
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}

#[tokio::test]
async fn editor_goal_and_constraints_are_frozen_at_prepare_not_read_later() {
    let (core, ctx, calls, server) = fixture().await;
    call(
        &core,
        &ctx,
        "config.update",
        json!({
            "task_goal": "goal A",
            "task_constraints": ["constraint one", "constraint two"],
            "editor_prompt": "prompt A",
            "editor_feedback": {"max_corrections": 0, "feedback_chars": 1024}
        }),
    )
    .await;
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"unit text"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Later host changes must not rewrite the frozen stage information.
    call(
        &core,
        &ctx,
        "config.update",
        json!({"task_goal":"goal B","task_constraints":["late constraint"]}),
    )
    .await;
    let run = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap();
    assert_eq!(run["result"]["status"], "ready");
    let first_calls = calls.lock().await;
    assert_eq!(first_calls.len(), 1);
    let text = first_calls[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    assert!(text.contains("goal A"), "frozen goal reaches the Editor I");
    assert!(
        text.contains("constraint one"),
        "frozen constraints reach the Editor I"
    );
    assert!(
        text.contains("prompt A"),
        "frozen prompt reaches the Editor I"
    );
    assert!(
        !text.contains("goal B"),
        "later host config must not leak in"
    );
    drop(first_calls);
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(
        state["jobs"][&job]["frozen_editor_config"]["task_goal"],
        "goal A"
    );
    assert_eq!(
        state["jobs"][&job]["frozen_editor_config"]["task_constraints"],
        json!(["constraint one", "constraint two"])
    );
    assert_eq!(
        state["jobs"][&job]["frozen_editor_config"]["editor_prompt"],
        "prompt A"
    );

    let revised = call(
        &core,
        &ctx,
        "edit.candidate.update",
        json!({"job_id":job,"expected_revision":0,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":"edited text"}],"pvi":{}}),
    )
    .await;
    assert_eq!(revised["revision"], 1);
    let cut_state = call(&core, &ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":cut_state["cut_seq"],"idempotency_key":"frozen-stage"}),
    )
    .await;
    let expanded = call(
        &core,
        &ctx,
        "hierarchy.expand",
        json!({"cb_id":published["cb_id"]}),
    )
    .await;
    assert_eq!(expanded["original_units"][0]["text"], "unit text");
    assert_eq!(expanded["original_units"][0]["anchor"]["source_id"], source);

    // A new stage after publication freezes the new host goal for the next
    // job, while the published job keeps its own frozen stage.
    call(
        &core,
        &ctx,
        "config.update",
        json!({"task_goal":"goal C","task_constraints":["stage two"]}),
    )
    .await;
    let second = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"second unit text"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[second]})).await;
    let second_job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":second,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_run = core
        .operate(
            "edit.run",
            &ctx,
            &json!({"job_id":second_job,"editor":"e1"}),
        )
        .await
        .unwrap();
    assert_eq!(second_run["result"]["status"], "ready");
    let calls = calls.lock().await;
    assert_eq!(calls.len(), 2);
    let second_text = calls[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    assert!(second_text.contains("goal C"), "new stage goal is used");
    assert!(second_text.contains("stage two"));
    assert!(!second_text.contains("goal A"), "old stage is not reused");
    drop(calls);
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(
        state["jobs"][&second_job]["frozen_editor_config"]["task_goal"],
        "goal C"
    );
    assert_eq!(
        state["jobs"][&job]["frozen_editor_config"]["task_goal"],
        "goal A"
    );
    server.abort();
}
