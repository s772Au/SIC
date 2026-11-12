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

async fn search_model(
    State(seen): State<Arc<Mutex<bool>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    *seen.lock().await = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"] == "sic_memory_search");
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"tool_calls":[{"id":"search-call","type":"function","function":{
            "name":"sic_memory_search","arguments":json!({"query":"needle"}).to_string()}}]}}]}),
    )
}

#[tokio::test]
async fn worker_receives_and_executes_the_authorized_memory_search_tool() {
    let seen = Arc::new(Mutex::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(search_model))
                .with_state(seen.clone()),
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
    let chain = op(&core, &ctx("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = ctx("alice", Some(&chain));
    let source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"evidence needle"}]}}),
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
    let response = op(&core, &alice, "worker.run", json!({})).await;
    assert!(*seen.lock().await);
    assert_eq!(response["tool_results"][0]["result"]["ok"], true);
    assert_eq!(
        response["tool_results"][0]["result"]["result"]["hits"][0]["cb_id"],
        cut["cb_id"]
    );
    assert_eq!(
        response["tool_results"][0]["result"]["result"]["hits"][0]["wb_id"],
        cut["wb_id"]
    );
    server.abort();
}

async fn scope_model(
    State(calls): State<Arc<Mutex<Vec<Value>>>>,
    Json(_body): Json<Value>,
) -> Json<Value> {
    Json(
        json!({"model":"mock-model","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","content":null,"tool_calls":calls.lock().await.clone()}}]}),
    )
}

#[tokio::test]
async fn worker_memory_tools_cannot_cross_into_another_internal_context() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(scope_model))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let p = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = op(&core, &ctx("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = ctx("alice", Some(&chain));
    let main_source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"public main fact"}]}}),
    )
    .await;
    let main_job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":main_source["source_id"],"rules":[]}),
    )
    .await;
    op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":main_job["job_id"],"expected_cut":0,"idempotency_key":"main"}),
    )
    .await;
    op(
        &core,
        &alice,
        "hierarchy.context.create",
        json!({"context_id":"editor-private"}),
    )
    .await;
    let private_source = op(&core, &alice, "hierarchy.context.ingest", json!({"context_id":"editor-private","request":{"messages":[{"role":"user","content":"private internal fact"}]}})).await;
    let private_job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":private_source["source_id"],"rules":[]}),
    )
    .await;
    let private_cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":private_job["job_id"],"expected_cut":1,"idempotency_key":"private"}),
    )
    .await;
    // The authenticated host can still inspect the Agent's other context.
    let host = op(
        &core,
        &alice,
        "memory.page",
        json!({"kind":"source","object_id":private_source["source_id"]}),
    )
    .await;
    assert!(
        host["value"]
            .as_str()
            .unwrap()
            .contains("private internal fact")
    );
    let make_call = |id: &str, name: &str, arguments: Value| json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}});
    *calls.lock().await = vec![
        make_call(
            "private-source",
            "sic_memory_page",
            json!({"kind":"source","object_id":private_source["source_id"]}),
        ),
        make_call(
            "private-cb",
            "sic_memory_replay",
            json!({"cb_id":private_cut["cb_id"]}),
        ),
        make_call(
            "private-wb",
            "sic_hierarchy_expand",
            json!({"wb_id":private_cut["wb_id"]}),
        ),
        make_call(
            "private-timeline",
            "sic_hierarchy_timeline",
            json!({"context_id":"editor-private","kind":"cb"}),
        ),
        make_call(
            "main-source",
            "sic_memory_page",
            json!({"kind":"source","object_id":main_source["source_id"]}),
        ),
        make_call(
            "main-timeline",
            "sic_hierarchy_timeline",
            json!({"kind":"cb"}),
        ),
    ];
    let worker = op(&core, &alice, "worker.run", json!({})).await;
    let results = worker["tool_results"].as_array().unwrap();
    for id in [
        "private-source",
        "private-cb",
        "private-wb",
        "private-timeline",
    ] {
        let result = results
            .iter()
            .find(|result| result["tool_call_id"] == id)
            .unwrap();
        assert_eq!(result["result"]["ok"], false);
        assert_eq!(result["result"]["code"], "forbidden");
    }
    let main = results
        .iter()
        .find(|result| result["tool_call_id"] == "main-source")
        .unwrap();
    assert_eq!(main["result"]["ok"], true);
    assert!(
        main["result"]["result"]["value"]
            .as_str()
            .unwrap()
            .contains("public main fact")
    );
    let timeline = results
        .iter()
        .find(|result| result["tool_call_id"] == "main-timeline")
        .unwrap();
    assert_eq!(timeline["result"]["result"]["context_id"], "main");
    assert!(
        !worker["tool_results"]
            .to_string()
            .contains("private internal fact")
    );
    server.abort();
}

#[tokio::test]
async fn worker_expansion_and_replay_are_paged_for_large_sources() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(scope_model))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = op(&core, &ctx("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = ctx("alice", Some(&chain));
    let long_text = "x".repeat(40_000);
    let messages: Vec<Value> = (0..6)
        .map(|_| json!({"role":"user","content":long_text}))
        .collect();
    let source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":messages}}),
    )
    .await;
    let job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":["deduplicate_exact"]}),
    )
    .await;
    let cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"large"}),
    )
    .await;
    let make_call = |id: &str, name: &str, arguments: Value| json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}});
    *calls.lock().await = vec![
        make_call(
            "expand-1",
            "sic_hierarchy_expand",
            json!({"cb_id":cut["cb_id"],"offset":0,"limit":3}),
        ),
        make_call(
            "expand-2",
            "sic_hierarchy_expand",
            json!({"cb_id":cut["cb_id"],"offset":3,"limit":3}),
        ),
        make_call(
            "replay-1",
            "sic_memory_replay",
            json!({"cb_id":cut["cb_id"],"offset":0,"limit":512}),
        ),
        make_call(
            "replay-2",
            "sic_memory_replay",
            json!({"cb_id":cut["cb_id"],"offset":512,"limit":512}),
        ),
        make_call(
            "bad-limit",
            "sic_hierarchy_expand",
            json!({"cb_id":cut["cb_id"],"limit":9}),
        ),
    ];
    let worker = op(&core, &alice, "worker.run", json!({})).await;
    let results = worker["tool_results"].as_array().unwrap();
    let value = |id: &str| {
        results
            .iter()
            .find(|result| result["tool_call_id"] == id)
            .unwrap()["result"]
            .clone()
    };
    let first = value("expand-1");
    assert_eq!(first["ok"], true);
    assert_eq!(first["result"]["unit_count"], 6);
    assert_eq!(first["result"]["next_offset"], 3);
    assert_eq!(
        first["result"]["original_units"][0]["text"]
            .as_str()
            .unwrap()
            .len(),
        512
    );
    assert_eq!(first["result"]["original_units"][0]["text_truncated"], true);
    assert!(value("expand-2")["result"]["next_offset"].is_null());
    let one = value("replay-1");
    let two = value("replay-2");
    assert_eq!(one["result"]["body_total"], 40_000);
    assert_eq!(one["result"]["next_offset"], 512);
    assert_eq!(two["result"]["offset"], 512);
    assert_eq!(one["result"]["body_digest"], two["result"]["body_digest"]);
    assert_eq!(one["result"]["body_matches_published_cb"], true);
    assert_eq!(value("bad-limit")["code"], "invalid");
    assert!(worker["tool_results"].to_string().len() < 15_000);
    let host = op(
        &core,
        &alice,
        "hierarchy.expand",
        json!({"cb_id":cut["cb_id"]}),
    )
    .await;
    assert_eq!(
        host["original_units"][0]["text"].as_str().unwrap().len(),
        40_000
    );
    server.abort();
}

#[tokio::test]
async fn worker_search_can_distinguish_pending_source_and_cb_body() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(scope_model))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = op(&core, &ctx("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = ctx("alice", Some(&chain));
    let first = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"first source"}]}}),
    )
    .await;
    let job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":first["source_id"],"rules":[],"pvi":{"P":["cb-only needle"]}}),
    )
    .await;
    let cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"p"}),
    )
    .await;
    let pending = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"pending needle"}]}}),
    )
    .await;
    let make_call = |id: &str, scope: &str| {
        json!({"id":id,"type":"function","function":{"name":"sic_memory_search",
        "arguments":json!({"query":"needle","scope":scope,"level":1}).to_string()}})
    };
    *calls.lock().await = vec![make_call("body", "cb"), make_call("pending", "pending")];
    let worker = op(&core, &alice, "worker.run", json!({})).await;
    let rows = worker["tool_results"].as_array().unwrap();
    let body = rows
        .iter()
        .find(|row| row["tool_call_id"] == "body")
        .unwrap();
    assert_eq!(body["result"]["result"]["hits"][0]["match_kind"], "cb_body");
    assert_eq!(body["result"]["result"]["hits"][0]["cb_id"], cut["cb_id"]);
    let pending_hit = rows
        .iter()
        .find(|row| row["tool_call_id"] == "pending")
        .unwrap();
    assert_eq!(
        pending_hit["result"]["result"]["hits"][0]["source_id"],
        pending["source_id"]
    );
    assert_eq!(
        pending_hit["result"]["result"]["hits"][0]["wb_source_membership"],
        "unformed"
    );
    assert!(pending_hit["result"]["result"]["hits"][0]["cb_id"].is_null());
    server.abort();
}
