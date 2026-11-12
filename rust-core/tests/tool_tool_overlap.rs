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
            "tool_calls":[
                {"id":"call-1","type":"function","function":{
                    "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"}},
                {"id":"call-2","type":"function","function":{
                    "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Kyoto\"}}"}}
            ]
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
async fn two_tool_intervals_are_counted_as_tool_tool_not_tool_model() {
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
        "default_output_tokens":128,"timeout_seconds":10});
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
        json!({"request":{"messages":[{"role":"user","content":"two weather lookups"}]}}),
    )
    .await;
    let worker = call(&core, &ctx, "worker.run", json!({})).await;
    assert_eq!(worker["host_tasks"].as_array().unwrap().len(), 2);
    let tasks = call(&core, &ctx, "tools.tasks", json!({})).await;
    let mut ids = Vec::new();
    for task in tasks["tasks"].as_array().unwrap() {
        let id = task["task_id"].as_str().unwrap().to_owned();
        call(
            &core,
            &ctx,
            "tool.claim",
            json!({"task_id":id,"claim_id":format!("claim-{id}")}),
        )
        .await;
        ids.push(id);
    }
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    for id in &ids {
        call(
            &core,
            &ctx,
            "tool.complete",
            json!({"task_id":id,"claim_id":format!("claim-{id}"),"succeeded":true,
                   "result":{"temperature":"20C"}}),
        )
        .await;
    }
    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let timing = &accounting["resources"]["timing"];
    assert_eq!(
        timing["intervals"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|interval| interval["role"] == "tool")
            .count(),
        2
    );
    assert!(
        timing["overlap"]["tool_tool"].as_u64().unwrap() >= 1,
        "{timing}"
    );
    assert_eq!(timing["overlap"]["tool_model"], 0);
    server.abort();
}
