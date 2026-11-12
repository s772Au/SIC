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
        "usage": {"prompt_tokens": 16, "completion_tokens": 8,
                  "prompt_tokens_details": {"cached_tokens": 7}},
        "choices": [{"finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "edit-empty", "type": "function",
                "function": {"name": "sic_edit_blocks", "arguments": "{\"ops\":[],\"pvi\":{}}"}}]
        }}]
    }))
}
fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":32000,"max_input_tokens":32000,
        "max_output_tokens":2048,"default_output_tokens":128,"timeout_seconds":5})
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

#[tokio::test]
async fn provider_cache_tokens_are_reported_without_claiming_physical_proof() {
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
    let store = Store::memory().await.unwrap();
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":p.clone(),"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"cache unit"}]}}),
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
    call(&core, &ctx, "edit.run", json!({"job_id":job,"editor":"e1"})).await;

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let attempts = accounting["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["provider_reported_cached_tokens"], 7);
    assert_eq!(attempts[0]["cache_observation"], "provider_reported");
    assert_eq!(accounting["provider_reported_cached_prompt_tokens"], 7);
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
