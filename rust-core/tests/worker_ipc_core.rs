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

async fn paged_message_model(
    State(calls): State<Arc<Mutex<Vec<Value>>>>,
    Json(_): Json<Value>,
) -> Json<Value> {
    Json(
        json!({"model":"mock-model","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"tool_calls":calls.lock().await.clone()}}]}),
    )
}

#[tokio::test]
async fn worker_message_pages_preserve_complete_text_and_respect_publication_and_route() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(paged_message_model))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{address}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let sender = chain(&core, "alice").await;
    let target = chain(&core, "bob").await;
    let other = chain(&core, "charlie").await;
    let alice = context("alice", Some(&sender));
    let bob = context("bob", Some(&target));
    let charlie = context("charlie", Some(&other));
    let initial = send_source(&core, &bob, "initial").await;
    publish(&core, &bob, &initial, "initial").await;
    let checkpoint = call(&core, &bob, "chain.checkpoint", json!({})).await;
    let socket = call(&core, &bob, "socket.create", json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{"message":[sender],"signal":[],"spawn":[],"read":[]}})).await;
    let long_text = "讯".repeat(3000);
    let sent = call(&core, &alice, "socket.message", json!({"socket_id":socket["socket_id"],"request_id":"q1","message_id":"long-message","text":long_text,"lifecycle":"P"})).await;
    let first_sequence = sent["receipt"]["sequence"].as_u64().unwrap();
    call(&core, &alice, "socket.message", json!({"socket_id":socket["socket_id"],"request_id":"q2","message_id":"short-message","text":"short clue","lifecycle":"P"})).await;
    let make_call = |id: &str, name: &str, arguments: Value| json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}});
    *calls.lock().await = vec![
        make_call(
            "not-yet",
            "sic_message_read",
            json!({"message_id":"long-message"}),
        ),
        make_call("before-list", "sic_message_receive", json!({})),
    ];
    let before = call(&core, &bob, "worker.run", json!({})).await;
    assert_eq!(before["tool_results"][0]["result"]["code"], "forbidden");
    assert_eq!(
        before["tool_results"][1]["result"]["result"]["messages"],
        json!([])
    );
    send_source(&core, &bob, "trigger publication").await;
    let state = call(&core, &bob, "chain.get", json!({})).await;
    let pending = state["contexts"]["main"]["levels"]["1"]["pending"].clone();
    let formed = call(&core, &bob, "wb.form", json!({"source_ids":pending})).await;
    publish(
        &core,
        &bob,
        formed["source_id"].as_str().unwrap(),
        "consume",
    )
    .await;
    *calls.lock().await = vec![
        make_call("list-1", "sic_message_receive", json!({"limit":1})),
        make_call(
            "list-2",
            "sic_message_receive",
            json!({"cursor":first_sequence.to_string(),"limit":1}),
        ),
        make_call(
            "read-1",
            "sic_message_read",
            json!({"message_id":"long-message","offset":0,"limit":1024}),
        ),
        make_call(
            "read-2",
            "sic_message_read",
            json!({"message_id":"long-message","offset":1024,"limit":1024}),
        ),
        make_call(
            "read-3",
            "sic_message_read",
            json!({"message_id":"long-message","offset":2048,"limit":1024}),
        ),
    ];
    let received = call(&core, &bob, "worker.run", json!({})).await;
    let rows = received["tool_results"].as_array().unwrap();
    let value = |id: &str| {
        rows.iter().find(|row| row["tool_call_id"] == id).unwrap()["result"]["result"].clone()
    };
    let first = value("list-1");
    assert_eq!(first["has_more"], true);
    assert_eq!(
        first["messages"][0]["message"]["message_id"],
        "long-message"
    );
    assert_eq!(
        first["messages"][0]["message"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        512
    );
    assert_eq!(first["messages"][0]["message"]["text_truncated"], true);
    assert_eq!(
        value("list-2")["messages"][0]["message"]["message_id"],
        "short-message"
    );
    let pages: Vec<String> = ["read-1", "read-2", "read-3"]
        .iter()
        .map(|id| value(id)["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(pages.concat(), long_text);
    assert!(value("read-3")["next_offset"].is_null());
    *calls.lock().await = vec![make_call(
        "foreign",
        "sic_message_read",
        json!({"message_id":"long-message"}),
    )];
    let foreign = call(&core, &charlie, "worker.run", json!({})).await;
    assert_eq!(foreign["tool_results"][0]["result"]["code"], "forbidden");
    assert!(
        !foreign["tool_results"]
            .to_string()
            .contains(&"讯".repeat(20))
    );
    server.abort();
}
