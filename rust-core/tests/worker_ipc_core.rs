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
    target: Arc<Mutex<String>>,
}
async fn model(State(fixture): State<Fixture>, Json(_): Json<Value>) -> Json<Value> {
    let target = fixture.target.lock().await.clone();
    let args = json!({"target":target,"text":"peer clue","lifecycle":"P"}).to_string();
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"tool_calls":[{"id":"message-call","type":"function",
        "function":{"name":"sic_message_send","arguments":args}}]}}]}),
    )
}
fn context(actor: &str, id: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: id.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, p: Value) -> Value {
    core.operate(op, ctx, &p).await.unwrap()["result"].clone()
}
async fn chain(core: &Core, actor: &str) -> String {
    call(core, &context(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn send_source(core: &Core, ctx: &OperationContext, s: &str) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":s}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn publish(core: &Core, ctx: &OperationContext, source: &str, key: &str) {
    let j = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let s = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":j["job_id"],"expected_cut":s["cut_seq"],"idempotency_key":key}),
    )
    .await;
}

#[tokio::test]
async fn worker_message_tool_obeys_socket_acl_and_arrives_at_next_cb() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let p = json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":format!("http://{address}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let sender = chain(&core, "alice").await;
    let target = chain(&core, "bob").await;
    let other = chain(&core, "charlie").await;
    let alice = context("alice", Some(&sender));
    let bob = context("bob", Some(&target));
    let charlie = context("charlie", Some(&other));
    let s = send_source(&core, &bob, "initial").await;
    publish(&core, &bob, &s, "initial").await;
    let cp = call(&core, &bob, "chain.checkpoint", json!({})).await;
    let socket=call(&core,&bob,"socket.create",json!({"checkpoint_id":cp["checkpoint_id"],"acl":{"message":[sender],"signal":[],"spawn":[],"read":[]}})).await;
    *fixture.target.lock().await = socket["socket_id"].as_str().unwrap().into();
    send_source(&core, &alice, "send clue").await;
    let sent = call(&core, &alice, "worker.run", json!({})).await;
    assert_eq!(sent["tool_results"][0]["result"]["ok"], true);
    let inbox = call(&core, &bob, "socket.inbox", json!({})).await;
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
    assert!(inbox["messages"][0]["consumed_by_cut"].is_null());
    send_source(&core, &charlie, "attempt unauthorized send").await;
    let rejected = call(&core, &charlie, "worker.run", json!({})).await;
    assert_eq!(rejected["tool_results"][0]["result"]["ok"], false);
    assert_eq!(
        call(&core, &bob, "socket.inbox", json!({})).await["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let trigger = send_source(&core, &bob, "next work").await;
    publish(&core, &bob, &trigger, "receive").await;
    let cut = call(&core, &bob, "cut.get", json!({})).await;
    assert_eq!(cut["view"]["cbs"][1]["body"], "next work\npeer clue");
    server.abort();
}
