use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Fixture {
    cb: Arc<Mutex<String>>,
}
async fn model(State(f): State<Fixture>, Json(_): Json<Value>) -> Json<Value> {
    let cb = f.cb.lock().await.clone();
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"tool_calls":[{"id":"memory-call","type":"function","function":{
            "name":"sic_hierarchy_expand","arguments":json!({"cb_id":cb}).to_string()}}]}}]}),
    )
}
fn ctx(actor: &str, id: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: id.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}
async fn op(c: &Core, x: &OperationContext, name: &str, p: Value) -> Value {
    c.operate(name, x, &p).await.unwrap()["result"].clone()
}

#[tokio::test]
async fn worker_can_expand_its_cb_but_cannot_read_another_agents_cb() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let p = json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let a = op(&core, &ctx("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = op(&core, &ctx("bob", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = ctx("alice", Some(&a));
    let bob = ctx("bob", Some(&b));
    let source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"unique fact"}]}}),
    )
    .await;
    let job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":[]}),
    )
    .await;
    let cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"p"}),
    )
    .await;
    *fixture.cb.lock().await = cut["cb_id"].as_str().unwrap().into();
    let result = op(&core, &alice, "worker.run", json!({})).await;
    assert_eq!(result["tool_results"][0]["result"]["ok"], true);
    assert_eq!(
        result["tool_results"][0]["result"]["result"]["original_units"][0]["text"],
        "unique fact"
    );
    let rejected = op(&core, &bob, "worker.run", json!({})).await;
    assert_eq!(rejected["tool_results"][0]["result"]["ok"], false);
    assert_eq!(rejected["tool_results"][0]["result"]["code"], "forbidden");
    server.abort();
}
