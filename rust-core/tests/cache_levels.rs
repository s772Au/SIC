use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<usize>>,
}

async fn complete(State(mock): State<Mock>, Json(_body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    *calls += 1;
    let cached = if *calls == 1 { 3 } else { 7 };
    drop(calls);
    Json(json!({"model":"mock",
    "usage":{"prompt_tokens":20,"completion_tokens":4,
             "prompt_tokens_details":{"cached_tokens":cached}},
    "choices":[{"finish_reason":"stop","message":{
        "role":"assistant","content":"answer"
    }}]}))
}
fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
        chain_id: chain,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
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
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
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
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn representation_switch_exposes_provider_cache_deltas_without_physical_proof() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn({
        let mock = mock.clone();
        async move {
            let _ = axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(complete))
                    .with_state(mock),
            )
            .await;
        }
    });
    let store = Store::memory().await.unwrap();
    let profile = json!({"model":"openai/mock","api_base":format!("http://{address}/v1"),
        "api_key_env":"FIXTURE_ONLY","tokenizer_model":"host-surrogate",
        "context_window":32000,"max_input_tokens":32000,"max_output_tokens":2048,
        "default_output_tokens":128,"timeout_seconds":5});
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let mut cbs = Vec::new();
    for n in 0..4 {
        cbs.push(publish(&core, &ctx, &format!("fact {n}"), &format!("cache-{n}")).await);
    }
    call(&core, &ctx, "worker.run", json!({})).await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":2,"cb_ids":cbs}),
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
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"cache-level"}),
    )
    .await;
    call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"context_id":"main","roles":["worker","editor"],"level":2}),
    )
    .await;
    call(&core, &ctx, "worker.run", json!({})).await;

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let attempts = accounting["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["provider_reported_cached_tokens"], 3);
    assert_eq!(attempts[1]["provider_reported_cached_tokens"], 7);
    assert_eq!(accounting["provider_reported_cached_prompt_tokens"], 10);
    assert_eq!(
        accounting["cache_observation"]["class"],
        "provider_reported"
    );
    assert_eq!(
        accounting["cache_observation"]["physical_cache_hit_proof"],
        false
    );
    assert_eq!(accounting["physical_cache_hit"], "unknown");
    assert_eq!(
        accounting["cache_observation"]["attempts_without_cache_report"],
        0
    );
    server.abort();
}
