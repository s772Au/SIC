use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use std::time::Duration;

async fn complete(Json(_body): Json<Value>) -> Json<Value> {
    Json(
        json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,
            "tool_calls":[{"id":"host-call-1","type":"function","function":{
                "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"
            }}]
        }}]}),
    )
}
fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "alice".into(),
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
async fn host_tool_task_records_created_claimed_and_completed_times() {
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
    call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"weather"}]}}),
    )
    .await;
    call(&core, &ctx, "worker.run", json!({})).await;
    let task = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0].clone();
    let task_id = task["task_id"].as_str().unwrap().to_owned();
    let created = task["created_at"].as_u64().unwrap();
    assert!(task["claimed_at"].is_null());

    call(
        &core,
        &ctx,
        "tool.claim",
        json!({"task_id":task_id,"claim_id":"host-claim"}),
    )
    .await;
    let claimed_task = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0].clone();
    let claimed = claimed_task["claimed_at"].as_u64().unwrap();
    assert!(claimed >= created);
    assert!(claimed_task["completed_at"].is_null());

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    call(
        &core,
        &ctx,
        "tool.complete",
        json!({"task_id":task_id,"claim_id":"host-claim","succeeded":true,
               "result":{"temperature":"20C"}}),
    )
    .await;
    let completed_task = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0].clone();
    let completed = completed_task["completed_at"].as_u64().unwrap();
    assert!(completed >= claimed + 1, "{completed_task}");
    assert_eq!(completed_task["status"], "completed");
    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let tools = &accounting["resources"]["tools"];
    assert_eq!(tools["tasks"]["completed"], 1);
    assert_eq!(tools["tasks"]["total"], 1);
    assert!(tools["execution_seconds"].as_u64().unwrap() >= 1);
    assert!(tools["max_execution_seconds"].as_u64().unwrap() >= 1);
    assert_eq!(tools["by_tool_duration"]["weather"]["completed"], 1);
    assert!(
        tools["by_tool_duration"]["weather"]["execution_seconds"]
            .as_u64()
            .unwrap()
            >= 1
    );
    server.abort();
}
