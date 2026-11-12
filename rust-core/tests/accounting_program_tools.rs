use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;

async fn complete(Json(_request): Json<Value>) -> Json<Value> {
    Json(
        json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,"tool_calls":[{"id":"host-call-1","type":"function","function":{
                "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"
            }}]
        }}]}),
    )
}
fn profile(endpoint: String) -> Value {
    json!({"model":"openai/mock","api_base":endpoint,"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":32000,"max_input_tokens":32000,
        "max_output_tokens":2048,"default_output_tokens":128,"timeout_seconds":3})
}
fn context(chain_id: Option<String>) -> OperationContext {
    OperationContext {
        actor: "alice".into(),
        chain_id,
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
async fn accounting_records_program_and_tool_dispatch_costs() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(()),
        )
        .await;
    });
    let store = Store::memory().await.unwrap();
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let grant = json!({"principal":"alice","tool_name":"weather"});
    call(
        &core,
        &ctx,
        "tools.configure",
        json!({"acl":{"discover":[grant.clone()],"expand":[grant.clone()],"execute":[grant]},
            "tools":[{"host_fixed":false,"descriptor":{"name":"weather","description":"weather lookup","schema":{
                "type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}]}),
    )
    .await;
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"weather"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        call(&core, &ctx, "worker.run", json!({})).await["host_tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let task = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "tool.claim",
        json!({"task_id":task,"claim_id":"host-claim"}),
    )
    .await;
    call(
        &core,
        &ctx,
        "tool.complete",
        json!({"task_id":task,"claim_id":"host-claim","succeeded":true,
               "result":{"temperature":"20C"}}),
    )
    .await;

    // A program edit/publish adds one published job on the same chain.
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"cost-cb"}),
    )
    .await;

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let program = &accounting["resources"]["program"];
    assert_eq!(program["class"], "host_measured");
    assert!(program["jobs_total"].as_u64().unwrap() >= 1);
    assert!(program["jobs_by_status"]["published"].as_u64().unwrap() >= 1);
    assert!(program["assignments"].as_u64().unwrap() >= 1);
    let tools = &accounting["resources"]["tools"];
    assert_eq!(tools["class"], "host_measured");
    assert_eq!(tools["dispatches"], 1);
    assert_eq!(tools["succeeded"], 1);
    assert_eq!(tools["by_tool"]["weather"]["calls"], 1);
    assert_eq!(tools["by_tool"]["weather"]["succeeded"], 1);
    assert!(tools["bytes"].as_u64().unwrap() > 0);
    assert!(tools["max_bytes"].as_u64().unwrap() > 0);
    assert!(tools["by_tool"]["weather"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(accounting["resources"]["adapter"]["class"], "not_available");
    assert!(accounting["resources"]["adapter"]["reason"].is_string());
    server.abort();
}
