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
    calls: Arc<Mutex<Vec<Value>>>,
}

async fn completion(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    if calls.len() >= 3 {
        return Json(
            json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":10,"completion_tokens":5},
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Fixture Worker answer"}}]}),
        );
    }
    let args = if calls.len() == 1 {
        json!({"ops":[],"pvi":{"P":[{"unexpected":"object"}]}})
    } else {
        json!({"ops":[],"pvi":{"V":["fresh volatile"]}})
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":10,"completion_tokens":5},
        "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":format!("call-{}",calls.len()),"type":"function","function":{
            "name":"sic_edit_blocks","arguments":args.to_string()}}]}}]}),
    )
}

fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"TEST_MOCK_KEY",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native",
        "context_window":1_000_000,"max_input_tokens":1_000_000,"max_output_tokens":393_216,
        "default_output_tokens":4096,"timeout_seconds":10})
}

async fn fail_then_worker(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    if calls.len() == 1 {
        return Json(
            json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,"tool_calls":[{"id":"bad","type":"function",
            "function":{"name":"sic_edit_blocks","arguments":"{invalid"}}]}}]}),
        );
    }
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"stop","message":{
        "role":"assistant","content":"Worker after replacement"}}]}),
    )
}

async fn wrong_tool_then_edit(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    let (name, arguments) = if calls.len() == 1 {
        ("sic_tool_search", json!({"query":"fixture"}))
    } else {
        ("sic_edit_blocks", json!({"ops":[],"pvi":{}}))
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls",
        "message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("call-{}",calls.len()),
        "type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]}),
    )
}

#[tokio::test]
async fn editor_recovers_from_wrong_common_tool_with_bounded_feedback() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(wrong_tool_then_edit))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(chain["result"]["chain_id"].as_str().unwrap().into());
    let source = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[
        {"role":"user","content":"search for the fixture and do the task"}]}}),
        )
        .await
        .unwrap();
    let job = core
        .operate(
            "edit.prepare",
            &context,
            &json!({"source_id":source["result"]["source_id"]}),
        )
        .await
        .unwrap();
    let result = core
        .operate(
            "edit.run",
            &context,
            &json!({"job_id":job["result"]["job_id"]}),
        )
        .await
        .unwrap();
    assert_eq!(result["result"]["attempts"], 2);
    let requests = mock.calls.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"], requests[1]["tools"]);
    assert!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Host validation rejected")
    );
    server.abort();
}

#[tokio::test]
async fn failed_editor_still_blocks_worker_until_a_replacement_is_published() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(fail_then_worker))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(chain["result"]["chain_id"].as_str().unwrap().into());
    core.operate(
        "config.update",
        &context,
        &json!({"editor_feedback":{"max_corrections":0,"feedback_chars":8192}}),
    )
    .await
    .unwrap();
    let source = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[{"role":"user","content":"task"}]}}),
        )
        .await
        .unwrap();
    let id = &source["result"]["source_id"];
    let job = core
        .operate("edit.prepare", &context, &json!({"source_id":id}))
        .await
        .unwrap();
    assert!(
        core.operate(
            "edit.run",
            &context,
            &json!({"job_id":job["result"]["job_id"]})
        )
        .await
        .is_err()
    );
    assert_eq!(
        core.operate("worker.run", &context, &json!({}))
            .await
            .unwrap_err()
            .code,
        "editor_pending"
    );
    assert_eq!(mock.calls.lock().await.len(), 1);
    let replacement = core
        .operate(
            "edit.program",
            &context,
            &json!({"source_id":id,"rules":[]}),
        )
        .await
        .unwrap();
    core.operate("cut.publish",&context,&json!({"job_id":replacement["result"]["job_id"],"expected_cut":0,"idempotency_key":"replacement"})).await.unwrap();
    let worker = core
        .operate("worker.run", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(worker["result"]["content"], "Worker after replacement");
    server.abort();
}

#[tokio::test]
async fn editor_invalid_candidate_gets_bounded_feedback_and_publishes_real_second_response() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let mut p = profile(format!("http://{address}/v1"));
    p["reasoning_effort"] = json!("low");
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let created = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(created["result"]["chain_id"].as_str().unwrap().into());
    let incoming = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[
        {"role":"system","content":"Protected harness instruction"},
        {"role":"user","content":"Important actual task"}]}}),
        )
        .await
        .unwrap();
    let prepared = core
        .operate(
            "edit.prepare",
            &context,
            &json!({"source_id":incoming["result"]["source_id"]}),
        )
        .await
        .unwrap();
    let job = prepared["result"]["job_id"].as_str().unwrap();
    assert_eq!(
        core.operate("worker.run", &context, &json!({}))
            .await
            .unwrap_err()
            .code,
        "editor_pending"
    );
    assert!(mock.calls.lock().await.is_empty());
    let run = core
        .operate("edit.run", &context, &json!({"job_id":job}))
        .await
        .unwrap();
    assert_eq!(run["result"]["status"], "ready");
    assert_eq!(run["result"]["attempts"], 2);
    assert_eq!(run["evidence"]["provider_called"], true);
    let requests = mock.calls.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["messages"][0]["content"],
        "Protected harness instruction"
    );
    assert_eq!(requests[0]["model"], "deepseek-v4.1-flash-eco");
    assert_eq!(requests[0]["reasoning_effort"], "low");
    assert_eq!(requests[0]["tools"], requests[1]["tools"]);
    assert!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Host validation rejected")
    );
    drop(requests);
    let published = core
        .operate(
            "cut.publish",
            &context,
            &json!({"job_id":job,"expected_cut":0,"idempotency_key":"publish"}),
        )
        .await
        .unwrap();
    assert_eq!(published["result"]["sequence"], 1);
    let cut = core.operate("cut.get", &context, &json!({})).await.unwrap();
    assert_eq!(
        cut["result"]["view"]["cbs"][0]["body"],
        "Important actual task"
    );
    let worker = core
        .operate("worker.run", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(worker["result"]["content"], "Fixture Worker answer");
    assert!(worker["result"]["source_id"].is_string());
    let pending = core.operate("cut.get", &context, &json!({})).await.unwrap();
    core.operate(
        "edit.prepare",
        &context,
        &json!({"source_id":pending["result"]["pending"][0]}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.operate("worker.run", &context, &json!({}))
            .await
            .unwrap_err()
            .code,
        "editor_pending"
    );
    let requests = mock.calls.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["tools"], requests[2]["tools"]);
    assert_eq!(requests[2]["reasoning_effort"], "low");
    assert_eq!(requests[0]["messages"][0], requests[2]["messages"][0]);
    assert_eq!(requests[0]["messages"][1], requests[2]["messages"][1]);
    assert!(
        requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["content"] == "Important actual task")
    );
    assert!(
        requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["content"] == "fresh volatile")
    );
    drop(requests);
    let usage = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(usage["result"]["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(usage["result"]["complete"], true);
    assert_eq!(usage["result"]["priced_cost"], Value::Null);
    server.abort();
}
