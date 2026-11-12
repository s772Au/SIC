use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;

async fn completion(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({
        "model": "deepseek-v4.1-flash-eco",
        "usage": {"prompt_tokens": 16, "completion_tokens": 8},
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "answer"}}]
    }))
}
async fn fixture() -> (Core, OperationContext, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(()),
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
    (core, context, server)
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn publish(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> String {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn accounting_separates_host_measured_resources_from_model_surrogates() {
    let (core, ctx, server) = fixture().await;
    let first = publish(&core, &ctx, "first source", "ledger-one").await;
    let second = publish(&core, &ctx, "second source", "ledger-two").await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":2,"cb_ids":[first, second]}),
    )
    .await;
    let job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":higher["source_id"],"rules":[]}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"ledger-higher"}),
    )
    .await;

    let before = call(&core, &ctx, "accounting.get", json!({})).await;
    assert_eq!(before["resources"]["storage"]["class"], "host_measured");
    assert!(before["resources"]["storage"]["bytes"].as_u64().unwrap() > 0);
    assert!(
        before["resources"]["storage"]["by_kind"]["source"]["objects"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert_eq!(
        before["resources"]["blocks"]["cbs"],
        before["resources"]["storage"]["by_kind"]["cb"]["objects"]
    );
    assert!(before["resources"]["events"]["count"].as_u64().unwrap() >= 2);
    assert!(before["resources"]["events"].get("first").is_some());
    assert!(before["resources"]["events"].get("last").is_some());
    assert_eq!(before["resources"]["model"]["class"], "host_surrogate");
    assert_eq!(before["resources"]["model"]["provider_billed"], false);
    assert_eq!(before["complete"], false);
    assert!(before["priced_cost"].is_null());

    // A real Worker call records provider usage, but the ledger still does not
    // claim a billed price and marks storage/blocks as host-measured.
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"question"}]}}),
    )
    .await;
    let source_id = source["source_id"].as_str().unwrap().to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source_id]})).await;
    core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let after = call(&core, &ctx, "accounting.get", json!({})).await;
    assert!(!after["attempts"].as_array().unwrap().is_empty());
    assert_eq!(after["known_prompt_tokens"], 16);
    assert_eq!(after["known_completion_tokens"], 8);
    assert_eq!(after["resources"]["model"]["provider_billed"], false);
    assert!(after["priced_cost"].is_null());
    assert_eq!(after["resources"]["storage"]["class"], "host_measured");
    let timing = &after["resources"]["timing"];
    let intervals = timing["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 1, "{timing}");
    assert_eq!(intervals[0]["role"], "worker");
    assert!(timing["overlap"]["worker_editor"].is_u64());
    assert!(timing["total_model_seconds"].is_u64());
    assert_eq!(
        timing["by_role"]["worker"].as_u64().unwrap(),
        timing["total_model_seconds"].as_u64().unwrap()
    );
    server.abort();
}
