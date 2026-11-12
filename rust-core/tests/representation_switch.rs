use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockProvider {
    calls: Arc<Mutex<Vec<Value>>>,
}

async fn completion(
    State(mock): State<MockProvider>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    mock.calls.lock().await.push(body);
    (
        StatusCode::OK,
        Json(json!({
            "model": "deepseek-v4.1-flash-eco",
            "usage": {"prompt_tokens": 16, "completion_tokens": 8},
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "answer"}}]
        })),
    )
}

async fn fixture() -> (
    Core,
    OperationContext,
    Arc<Mutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(mock),
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

async fn ingest(core: &Core, ctx: &OperationContext, text: &str) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish(core: &Core, ctx: &OperationContext, source: &str, key: &str) -> String {
    let prepared = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn form_and_publish(
    core: &Core,
    ctx: &OperationContext,
    level: u32,
    members: &[String],
    key: &str,
) -> String {
    let formed = call(
        core,
        ctx,
        "hierarchy.form",
        json!({"level":level,"cb_ids":members}),
    )
    .await;
    let source = formed["source_id"].as_str().unwrap().to_owned();
    publish(core, ctx, &source, key).await
}

#[tokio::test]
async fn switching_the_active_representation_changes_and_restores_the_logical_prefix() {
    let (core, ctx, calls, server) = fixture().await;
    let mut ones = Vec::new();
    for n in 0..4 {
        let source = ingest(&core, &ctx, &format!("fact {n}")).await;
        ones.push(publish(&core, &ctx, &source, &format!("base-{n}")).await);
    }
    let two_a =
        form_and_publish(&core, &ctx, 2, &[ones[0].clone(), ones[1].clone()], "two-a").await;
    let two_b =
        form_and_publish(&core, &ctx, 2, &[ones[2].clone(), ones[3].clone()], "two-b").await;
    let three = form_and_publish(&core, &ctx, 3, &[two_a.clone(), two_b.clone()], "three").await;

    // Level 3 is a single CB; level 1 is the four original blocks.
    let high = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"level":3,"roles":["worker","editor"]}),
    )
    .await;
    assert_eq!(high["role_views"]["worker"]["level"], 3);
    assert_eq!(
        call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let ingest_and_run = |question: &str| {
        let core = core.clone();
        let ctx = ctx.clone();
        let question = question.to_owned();
        async move {
            ingest(&core, &ctx, &question).await;
            let run = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
            run["evidence"]["common_prefix_digest"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let high_digest = ingest_and_run("question high one").await;

    let low = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"level":1,"roles":["worker","editor"]}),
    )
    .await;
    assert_eq!(low["role_views"]["worker"]["level"], 1);
    let low_view = call(&core, &ctx, "cut.get", json!({})).await;
    assert_eq!(low_view["view"]["cbs"].as_array().unwrap().len(), 4);
    let low_digest = ingest_and_run("question low one").await;
    assert_ne!(
        high_digest, low_digest,
        "the active representation is part of the logical prefix"
    );

    let again = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"level":3,"roles":["worker","editor"]}),
    )
    .await;
    assert_eq!(again["role_views"]["worker"]["level"], 3);
    let restored = ingest_and_run("question high two").await;
    assert_eq!(
        restored, high_digest,
        "switching back restores the same logical prefix digest"
    );

    let calls = calls.lock().await;
    assert_eq!(calls.len(), 3, "one Worker call per run");
    let high_request = serde_json::to_string(&calls[0]).unwrap();
    let low_request = serde_json::to_string(&calls[1]).unwrap();
    assert!(
        high_request.contains(&three),
        "level-3 CB body is used: {high_request}"
    );
    assert!(
        !low_request.contains(&three),
        "the level-1 request cannot silently reuse the level-3 block"
    );
    assert_eq!(again["selected_cb_ids"]["worker"], json!([three]));
    server.abort();
}
