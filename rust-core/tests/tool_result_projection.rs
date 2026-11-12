use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::{self, Store},
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Fixture {
    requests: Arc<Mutex<Vec<Value>>>,
    source_id: Arc<Mutex<String>>,
    read_offset: Arc<Mutex<u64>>,
}

async fn completion(State(fixture): State<Fixture>, Json(request): Json<Value>) -> Json<Value> {
    let count = {
        let mut requests = fixture.requests.lock().await;
        requests.push(request);
        requests.len()
    };
    let (finish, message) = if count == 1 {
        let source_id = fixture.source_id.lock().await.clone();
        let offset = *fixture.read_offset.lock().await;
        (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{
            "id":"read-original","type":"function","function":{"name":"sic_source_text_page",
                "arguments":json!({"source_id":source_id,"unit_id":"u1","offset":offset,"limit":512}).to_string()}}]}),
        )
    } else {
        ("stop", json!({"role":"assistant","content":"continued"}))
    };
    Json(json!({"model":"mock-model","choices":[{"finish_reason":finish,"message":message}]}))
}

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

async fn op(core: &Core, ctx: &OperationContext, name: &str, args: Value) -> Value {
    core.operate(name, ctx, &args).await.unwrap()["result"].clone()
}

#[tokio::test]
async fn large_tool_result_projection_preserves_original_and_can_be_disabled() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("synthetic-key".into())),
    )
    .unwrap();
    let chain = op(&core, &context("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = context("alice", Some(&chain));
    let content = "A讯🙂".repeat(30_000);
    let source = op(&core, &alice, "source.ingest", json!({"request":{"messages":[
        {"role":"assistant","content":null,"tool_calls":[{"id":"native-1","type":"function","function":{"name":"sic_tool_call","arguments":"{\"name\":\"fixture\",\"arguments\":{}}"}}]},
        {"role":"tool","tool_call_id":"native-1","content":content}
    ]}})).await;
    let source_id = source["source_id"].as_str().unwrap().to_owned();
    *fixture.source_id.lock().await = source_id.clone();
    *fixture.read_offset.lock().await = 50_000;
    let worker = op(&core, &alice, "worker.run", json!({})).await;
    let page = &worker["tool_results"][0]["result"]["result"];
    assert_eq!(page["source_id"], source_id);
    assert_eq!(
        page["text"].as_str().unwrap(),
        content.chars().skip(50_000).take(512).collect::<String>()
    );
    assert_eq!(page["full_digest"], storage::sha256(content.as_bytes()));
    let requests = fixture.requests.lock().await;
    let native = requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "native-1")
        .unwrap();
    let projected: Value = serde_json::from_str(native["content"].as_str().unwrap()).unwrap();
    let preview = &projected["sic_tool_result_preview"];
    assert_eq!(preview["total_chars"], 90_000);
    assert_eq!(preview["preview"].as_str().unwrap().chars().count(), 32_768);
    assert_eq!(preview["source_id"], source_id);
    assert_eq!(preview["unit_id"], "u1");
    assert_eq!(preview["full_digest"], storage::sha256(content.as_bytes()));
    drop(requests);
    let archived = op(&core, &alice, "source.read", json!({"source_id":source_id})).await;
    assert_eq!(
        archived["source"]["sidecar"]["messages"][1]["content"],
        content
    );
    let state = op(&core, &alice, "chain.get", json!({})).await;
    let run_id = state["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "worker_run_started")
        .unwrap()["record_id"]
        .as_str()
        .unwrap();
    let mut tx = store.begin().await.unwrap();
    let run = storage::get_object(&mut tx, run_id, Some("worker_run"))
        .await
        .unwrap();
    assert_eq!(run["tool_result_preview_chars"], 32_768);
    assert_eq!(run["tool_result_total_preview_chars"], 131_072);
    assert_eq!(
        run["tool_result_projections"][0]["full_digest"],
        storage::sha256(content.as_bytes())
    );
    assert!(run["tool_result_projections"][0].get("preview").is_none());
    tx.commit().await.unwrap();
    assert_eq!(
        core.operate(
            "source.text",
            &context("bob", Some(&chain)),
            &json!({"source_id":source_id,"unit_id":"u1"})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
    assert_eq!(
        core.operate(
            "config.update",
            &alice,
            &json!({"tool_result_preview_chars":1})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    op(
        &core,
        &alice,
        "config.update",
        json!({"tool_result_preview_chars":0}),
    )
    .await;
    op(&core, &alice, "worker.run", json!({})).await;
    let requests = fixture.requests.lock().await;
    let native = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "native-1")
        .unwrap();
    assert_eq!(native["content"], content);
    server.abort();
}

#[tokio::test]
async fn aggregate_budget_prioritizes_latest_tool_results_and_newly_read_pages() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"MOCK",
        "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,"default_output_tokens":4096});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("synthetic-key".into())),
    )
    .unwrap();
    let chain = op(&core, &context("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = context("alice", Some(&chain));
    op(
        &core,
        &alice,
        "config.update",
        json!({"tool_result_preview_chars":2048,"tool_result_total_preview_chars":4096}),
    )
    .await;
    let native_calls: Vec<Value> = (0..5)
        .map(|index| {
            json!({"id":format!("call-{index}"),"type":"function",
        "function":{"name":"sic_tool_call","arguments":"{\"name\":\"fixture\",\"arguments\":{}}"}})
        })
        .collect();
    let mut messages = vec![json!({"role":"assistant","content":null,"tool_calls":native_calls})];
    for index in 0..5 {
        messages.push(json!({"role":"tool","tool_call_id":format!("call-{index}"),
            "content":format!("marker-{index}:{}", "x".repeat(40_000))}));
    }
    let source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":messages}}),
    )
    .await;
    *fixture.source_id.lock().await = source["source_id"].as_str().unwrap().to_owned();
    op(&core, &alice, "worker.run", json!({})).await;
    op(&core, &alice, "worker.run", json!({})).await;
    let requests = fixture.requests.lock().await;
    let tool_messages: Vec<&Value> = requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect();
    assert_eq!(tool_messages.len(), 5);
    let previews: Vec<Value> = tool_messages.iter().map(|message| serde_json::from_str::<Value>(message["content"].as_str().unwrap()).unwrap()["sic_tool_result_preview"].clone()).collect();
    assert_eq!(
        previews
            .iter()
            .map(|preview| preview["preview_chars"].as_u64().unwrap())
            .sum::<u64>(),
        4096
    );
    assert_eq!(
        previews
            .iter()
            .map(|preview| preview["preview_chars"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![0, 0, 0, 2048, 2048]
    );
    let newest_page = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "read-original")
        .unwrap();
    let page_content = newest_page["content"].as_str().unwrap();
    assert!(page_content.contains("marker-0:"));
    assert!(!page_content.contains("sic_tool_result_preview"));
    server.abort();
}
