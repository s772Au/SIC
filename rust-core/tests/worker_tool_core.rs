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

async fn duplicate_indexes_model(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    requests.push(body);
    let (finish, message) = if requests.len() == 1 {
        (
            "tool_calls",
            json!({"role":"assistant","content":null,"reasoning_content":"opaque synthetic provider reasoning","tool_calls":[
                {"index":0,"id":"search-multi","type":"function","function":{"name":"sic_tool_search","arguments":"{\"query\":\"calculator\"}"}},
                {"index":0,"id":"expand-multi","type":"function","function":{"name":"sic_tool_expand","arguments":"{\"name\":\"calculator\"}"}}
            ]}),
        )
    } else {
        (
            "stop",
            json!({"role":"assistant","content":"parallel tools completed"}),
        )
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":5,"completion_tokens":2},
        "choices":[{"finish_reason":finish,"message":message}]}),
    )
}

async fn continuous_tool_model(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    requests.push(body);
    let n = requests.len();
    // Asking again for an already-expanded schema is not an execution and
    // must not keep an unused tool alive indefinitely.
    let (name, args) = if n == 1 || n == 8 {
        ("sic_tool_expand", json!({"name":"calculator"}))
    } else {
        ("sic_tool_search", json!({"query":"calculator"}))
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls",
        "message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("idle-test-{n}"),
        "type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}),
    )
}

async fn malformed_multiple_choice_model(
    State(mock): State<Mock>,
    Json(body): Json<Value>,
) -> Json<Value> {
    mock.requests.lock().await.push(body);
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":9,"completion_tokens":4},"choices":[
            {"index":0,"message":{"role":"assistant","content":"Synthetic incomplete candidate."}},
            {"index":1,"finish_reason":"tool_calls","message":{"role":"assistant","tool_calls":[
                {"id":"ambiguous-call","type":"function","function":{"name":"sic_tool_call",
                    "arguments":json!({"name":"calculator","arguments":{}}).to_string()}}
            ]}}
        ]}),
    )
}

#[tokio::test]
async fn ambiguous_provider_choices_are_audited_without_dispatching_a_tool() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(malformed_multiple_choice_model),
                )
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{addr}/v1"));
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
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
            .to_owned(),
    );
    let grant = json!({"principal":"alice","tool_name":"calculator"});
    core.operate("tools.configure", &ctx, &json!({"acl":{"discover":[grant],"expand":[grant],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"calculator","description":"fixture calculator",
            "schema":{"type":"object","properties":{}}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"fixture"}]}}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.operate("worker.run", &ctx, &json!({}))
            .await
            .unwrap_err()
            .code,
        "worker_invalid_choice_count"
    );
    assert_eq!(mock.requests.lock().await.len(), 1);
    let tasks = core.operate("tools.tasks", &ctx, &json!({})).await.unwrap();
    assert_eq!(tasks["result"]["tasks"], json!([]));
    let ledger = core
        .operate("accounting.get", &ctx, &json!({}))
        .await
        .unwrap();
    assert_eq!(ledger["result"]["known_prompt_tokens"], 9);
    assert_eq!(ledger["result"]["known_completion_tokens"], 4);
    let attempt = ledger["result"]["attempts"][0]["record_id"]
        .as_str()
        .unwrap();
    let mut tx = store.begin().await.unwrap();
    let audit = sic_core_rust::storage::get_object(&mut tx, attempt, Some("worker_attempt"))
        .await
        .unwrap();
    assert_eq!(
        audit["raw_response"]["choices"].as_array().unwrap().len(),
        2
    );
    assert!(audit["source_id"].is_null());
    tx.commit().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn unused_tool_is_evicted_after_fifteen_tool_call_responses() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(continuous_tool_model))
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
    core.operate("tools.configure", &ctx, &json!({"acl":{"discover":[grant],"expand":[grant],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"calculator","description":"fixture calculator",
            "schema":{"type":"object","properties":{}}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"find calculator"}]}}),
    )
    .await
    .unwrap();

    let expanded = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(expanded["result"]["tool_results"][0]["result"]["ok"], true);
    for _ in 0..14 {
        core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    }
    let status = core
        .operate("tools.status", &ctx, &json!({}))
        .await
        .unwrap();
    assert_eq!(status["result"]["catalog"][0]["expanded"], true);
    core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let status = core
        .operate("tools.status", &ctx, &json!({}))
        .await
        .unwrap();
    assert_eq!(status["result"]["catalog"][0]["expanded"], false);
    core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let requests = mock.requests.lock().await;
    let has_calculator = |request: &Value| {
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["content"]
                    .as_str()
                    .and_then(|content| serde_json::from_str::<Value>(content).ok())
                    .and_then(|view| {
                        view.pointer("/sic_editable_I_views/dynamic_tool_directory")
                            .cloned()
                    })
                    .and_then(|directory| directory.as_array().cloned())
                    .is_some_and(|directory| {
                        directory.iter().any(|tool| tool["name"] == "calculator")
                    })
            })
    };
    assert!(has_calculator(&requests[1]));
    assert!(!has_calculator(&requests[16]));
    server.abort();
}
fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"MOCK",
    "tokenizer_model":"host-o200k","context_window":1_000_000,"max_input_tokens":1_000_000,
    "max_output_tokens":393_216,"default_output_tokens":4096,"timeout_seconds":5})
}

#[tokio::test]
async fn complete_parallel_calls_with_repeated_indexes_are_dispatched_and_audited() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(duplicate_indexes_model))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{addr}/v1"));
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
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
    core.operate("tools.configure", &ctx, &json!({"acl":{"discover":[grant],"expand":[grant],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"calculator","description":"fixture calculator",
            "schema":{"type":"object","properties":{}}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"find calculator"}]}}),
    )
    .await
    .unwrap();
    let first = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(first["result"]["tool_calls"][0]["index"], 0);
    assert_eq!(first["result"]["tool_calls"][1]["index"], 1);
    assert_eq!(
        first["result"]["tool_results"][0]["tool_call_id"],
        "search-multi"
    );
    assert_eq!(
        first["result"]["tool_results"][1]["tool_call_id"],
        "expand-multi"
    );
    assert_eq!(first["result"]["tool_results"][1]["result"]["ok"], true);
    let mut tx = store.begin().await.unwrap();
    let raw = sic_core_rust::storage::get_object(
        &mut tx,
        first["result"]["attempt_id"].as_str().unwrap(),
        Some("worker_attempt"),
    )
    .await
    .unwrap();
    assert_eq!(
        raw["raw_response"]["choices"][0]["message"]["tool_calls"][1]["index"],
        0
    );
    tx.commit().await.unwrap();
    let next = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(next["result"]["content"], "parallel tools completed");
    let requests = mock.requests.lock().await;
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|m| m["role"] == "assistant"
        && m["reasoning_content"] == "opaque synthetic provider reasoning"
        && m["tool_calls"][1]["id"] == "expand-multi"));
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "search-multi")
    );
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "tool" && m["tool_call_id"] == "expand-multi")
    );
    let group = core
        .operate(
            "source.read",
            &ctx,
            &json!({"source_id":first["result"]["source_id"],"group":true}),
        )
        .await
        .unwrap();
    assert!(
        group["result"]["source"]["open_calls"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{group}"
    );
    let schedule = core
        .operate(
            "runtime.schedule",
            &ctx,
            &json!({"context_id":"main","min_sources_per_wb":1,
                "max_sources_per_wb":8,"max_units_per_wb":64,
                "level_batch_sizes":{},"max_level":1,"flush":true,
                "max_actions":64,"max_model_calls":0,
                "editor_mode":"program_fixture","program_rules":[]}),
        )
        .await
        .unwrap();
    assert_eq!(schedule["result"]["completed"], true, "{schedule}");
    server.abort();
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
