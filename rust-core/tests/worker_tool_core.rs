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
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn model(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    requests.push(body);
    let n = requests.len();
    let (finish, message) = match n {
        1 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"search-1","type":"function","function":{
            "name":"sic_tool_search","arguments":json!({"query":"calcul"}).to_string()}}]}),
        ),
        2 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{
            "name":"sic_tool_call","arguments":json!({"name":"calculator","arguments":{"a":2,"b":3}}).to_string()}}]}),
        ),
        _ => ("stop", json!({"role":"assistant","content":"5"})),
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":finish,"message":message}]}),
    )
}

async fn paged_catalog_model(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    requests.push(body);
    let n = requests.len();
    let (finish_reason, message) = if n == 1 {
        (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"page-search","type":"function","function":{
                "name":"sic_tool_search","arguments":json!({"query":"catalog","offset":100,"limit":100}).to_string()
            }}]}),
        )
    } else {
        (
            "stop",
            json!({"role":"assistant","content":"Catalog page received."}),
        )
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":finish_reason,"message":message}]}),
    )
}
fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"MOCK",
    "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,
    "max_output_tokens":393_216,"default_output_tokens":4096,"timeout_seconds":5})
}

#[tokio::test]
async fn real_core_dispatches_search_then_waits_for_host_tool_and_continues() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{addr}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("mock-key".into())),
    )
    .unwrap();
    let mut ctx = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    ctx.chain_id = Some(
        core.operate("chain.create", &ctx, &json!({}))
            .await
            .unwrap()["result"]["chain_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    let grant = json!({"principal":"alice","tool_name":"calculator"});
    core.operate("tools.configure",&ctx,&json!({"acl":{"discover":[grant],"expand":[grant],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"calculator","description":"calculate a sum","schema":{
            "type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"What is 2 plus 3?"}]}}),
    )
    .await
    .unwrap();
    let search = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(
        search["result"]["tool_results"][0]["result"]["tools"][0]["name"],
        "calculator"
    );
    assert_eq!(search["result"]["tool_results"][0]["result"]["offset"], 0);
    assert_eq!(search["result"]["tool_results"][0]["result"]["total"], 1);
    assert_eq!(
        search["result"]["tool_results"][0]["result"]["next_offset"],
        Value::Null
    );
    let requested = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(
        requested["result"]["host_tasks"][0]["descriptor"]["name"],
        "calculator"
    );
    assert_eq!(
        requested["result"]["host_tasks"][0]["arguments"],
        json!({"a":2,"b":3})
    );
    assert_eq!(
        core.operate("worker.run", &ctx, &json!({}))
            .await
            .unwrap_err()
            .code,
        "tool_group_open"
    );
    let payload = json!({"task_id":requested["result"]["host_tasks"][0]["task_id"],"succeeded":null,"result":5});
    let complete = core.operate("tool.complete", &ctx, &payload).await.unwrap();
    assert_eq!(complete["result"]["open_calls"], json!([]));
    assert_eq!(
        core.operate("tool.complete", &ctx, &payload).await.unwrap()["evidence"]["replayed"],
        true
    );
    assert!(
        core.operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":payload["task_id"],"succeeded":true,"result":6})
        )
        .await
        .is_err()
    );
    let answer = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(answer["result"]["content"], "5");
    let requests = mock.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["tools"], requests[2]["tools"]);
    assert!(
        requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "tool"
                    && message["tool_call_id"] == "search-1"
                    && serde_json::from_str::<Value>(message["content"].as_str().unwrap_or(""))
                        .is_ok_and(|content| {
                            content["total"] == 1
                                && content["offset"] == 0
                                && content["next_offset"].is_null()
                        })
            })
    );
    assert!(
        requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "system"
                && message["content"]
                    .as_str()
                    .unwrap_or("")
                    .contains("calculator"))
    );
    assert!(
        requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["tool_call_id"] == "call-1"
                && message["content"]
                    .as_str()
                    .unwrap_or("")
                    .contains("\"ok\":null"))
    );
    let status = core
        .operate("tools.status", &ctx, &json!({}))
        .await
        .unwrap();
    assert_eq!(status["result"]["catalog"][0]["expanded"], true);
    assert_eq!(status["result"]["active_leases"], json!([]));
    server.abort();
}

#[tokio::test]
async fn worker_i_bounds_large_catalog_and_search_pages_reach_remaining_tools() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(paged_catalog_model))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{addr}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("mock-key".into())),
    )
    .unwrap();
    let mut ctx = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    ctx.chain_id = Some(
        core.operate("chain.create", &ctx, &json!({}))
            .await
            .unwrap()["result"]["chain_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    let tools = (0..130)
        .map(|index| {
            json!({"host_fixed":false,"descriptor":{
                "name":format!("catalog_{index:03}"),
                "description":format!("unique_catalog_description_{index:03}"),
                "schema":{"type":"object","properties":{},"additionalProperties":false}
            }})
        })
        .collect::<Vec<_>>();
    let discover_acl = (0..130)
        .map(|index| json!({"principal":"alice","tool_name":format!("catalog_{index:03}")}))
        .collect::<Vec<_>>();
    core.operate(
        "tools.configure",
        &ctx,
        &json!({"acl":{"discover":discover_acl,"expand":[],"execute":[]},"tools":tools}),
    )
    .await
    .unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"List the available tools."}]}}),
    )
    .await
    .unwrap();

    let searched = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let result = &searched["result"]["tool_results"][0]["result"];
    assert_eq!(result["offset"], 100);
    assert_eq!(result["total"], 130);
    assert_eq!(result["next_offset"], Value::Null);
    assert_eq!(result["tools"].as_array().unwrap().len(), 30);
    assert_eq!(result["tools"][0]["name"], "catalog_100");
    assert_eq!(result["tools"][29]["name"], "catalog_129");

    core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let requests = mock.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let initial_context = requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(initial_context.contains("\"total\":130"));
    assert!(initial_context.contains("\"has_more\":true"));
    assert!(initial_context.contains("catalog_000"));
    assert!(initial_context.contains("catalog_019"));
    assert!(!initial_context.contains("catalog_020"));
    assert!(!initial_context.contains("catalog_129"));
    assert!(!initial_context.contains("unique_catalog_description_"));
    server.abort();
}
