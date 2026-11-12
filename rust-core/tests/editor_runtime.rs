use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<Vec<Value>>>,
    correction_started: Arc<Notify>,
    correction_release: Arc<Notify>,
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
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":7,"completion_tokens":3},"choices":[{"finish_reason":"tool_calls",
        "message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("call-{}",calls.len()),
        "type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]}),
    )
}

async fn invalid_choice_then_edit(
    State(mock): State<Mock>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    let valid = json!({"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"editor-valid","type":"function","function":{"name":"sic_edit_blocks",
        "arguments":"{\"ops\":[],\"pvi\":{}}"}}]}});
    let choices = if calls.len() == 1 {
        vec![
            json!({"index":0,"message":{"role":"assistant","content":"synthetic ambiguous choice"}}),
            valid,
        ]
    } else {
        vec![valid]
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":7,"completion_tokens":3},"choices":choices}),
    )
}

#[tokio::test]
async fn editor_rejects_multiple_choices_then_uses_bounded_feedback() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(invalid_choice_then_edit))
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
    context.chain_id = Some(
        core.operate("chain.create", &context, &json!({}))
            .await
            .unwrap()["result"]["chain_id"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let source = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[{"role":"user","content":"fixture"}]}}),
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
    assert_eq!(result["result"]["status"], "ready");
    assert_eq!(result["result"]["attempts"], 2);
    let calls = mock.calls.lock().await;
    assert_eq!(calls.len(), 2);
    let feedback = calls[1]["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(feedback.contains("exactly one choice"));
    assert!(!feedback.contains("synthetic ambiguous choice"));
    let ledger = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(ledger["result"]["known_prompt_tokens"], 14);
    assert_eq!(ledger["result"]["known_completion_tokens"], 6);
    server.abort();
}

async fn gated_editor_correction(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let n = {
        let mut calls = mock.calls.lock().await;
        calls.push(body);
        calls.len()
    };
    if n == 2 {
        mock.correction_started.notify_one();
        mock.correction_release.notified().await;
    }
    let arguments = if n == 1 {
        json!({"ops":[],"pvi":{"P":[{"invalid":"object"}]}})
    } else {
        json!({"ops":[],"pvi":{}})
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":7,"completion_tokens":3},
        "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{
            "id":format!("editor-correction-{n}"),"type":"function",
            "function":{"name":"sic_edit_blocks","arguments":arguments.to_string()}}]}}]}),
    )
}

#[tokio::test]
async fn editor_feedback_retry_is_unknown_until_its_response_arrives() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(gated_editor_correction))
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
            &json!({"request":{"messages":[{"role":"user","content":"retry fixture"}]}}),
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
    let running_core = core.clone();
    let running_context = context.clone();
    let job_id = job["result"]["job_id"].as_str().unwrap().to_owned();
    let running = tokio::spawn(async move {
        running_core
            .operate("edit.run", &running_context, &json!({"job_id":job_id}))
            .await
    });
    mock.correction_started.notified().await;
    let pending = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    let groups: Vec<_> = pending["result"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|g| g["role"] == "editor")
        .collect();
    assert_eq!(
        groups
            .iter()
            .map(|g| g["attempts"].as_u64().unwrap())
            .sum::<u64>(),
        2
    );
    assert_eq!(
        groups
            .iter()
            .map(|g| g["prompt_tokens"]["unknown_attempts"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    assert_eq!(pending["result"]["complete"], false);
    mock.correction_release.notify_one();
    running.await.unwrap().unwrap();
    let final_ledger = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    let group = &final_ledger["result"]["groups"][0];
    assert_eq!(group["attempts"], 2);
    assert_eq!(group["prompt_tokens"]["known_tokens"], 14);
    assert_eq!(group["completion_tokens"]["known_tokens"], 6);
    server.abort();
}

async fn truncated_worker() -> Json<Value> {
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":11,"completion_tokens":9},
        "choices":[{"finish_reason":"length","message":{"role":"assistant","content":"incomplete"}}]}),
    )
}

async fn worker_then_editor(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    let message = if calls.len() % 2 == 1 {
        json!({"finish_reason":"stop","message":{"role":"assistant","content":"worker completed"}})
    } else {
        json!({"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[{"id":format!("edit-{}",calls.len()),"type":"function",
            "function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]}})
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","usage":{"prompt_tokens":3,"completion_tokens":1},"choices":[message]}),
    )
}

#[tokio::test]
async fn shared_prefix_scope_distinguishes_route_and_unknown_cache_from_identical_text() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/same/chat/completions", post(worker_then_editor))
                .route("/other/chat/completions", post(worker_then_editor))
                .with_state(mock),
        )
        .into_future(),
    );

    // All cases use the same model and same logical CB/messages. Only the
    // host-declared cache scope, provider route, or control differs.
    for (different_route, declare_namespace, different_controls, scope_matches) in [
        (false, true, false, true),
        (true, true, false, false),
        (false, false, false, false),
        (false, true, true, false),
    ] {
        let mut worker = profile(format!("http://{address}/same"));
        let mut editor = profile(format!(
            "http://{address}/{}",
            if different_route { "other" } else { "same" }
        ));
        if declare_namespace {
            worker["cache_namespace"] = json!("fixture-declared-scope");
            editor["cache_namespace"] = json!("fixture-declared-scope");
        }
        if different_controls {
            editor["reasoning_effort"] = json!("low");
        }
        let core = Core::with_model_config_and_resolver(
            Store::memory().await.unwrap(),
            json!({"roles":{"worker":worker,"editor":editor}}),
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
                &json!({"request":{"messages":[{"role":"user","content":"scope fixture"}]}}),
            )
            .await
            .unwrap();
        let work = core
            .operate("worker.run", &context, &json!({}))
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
        let edit = core
            .operate(
                "edit.run",
                &context,
                &json!({"job_id":job["result"]["job_id"]}),
            )
            .await
            .unwrap();
        assert_eq!(
            work["evidence"]["common_prefix_digest"],
            edit["evidence"]["common_prefix_digest"]
        );
        assert_eq!(
            work["evidence"]["prefix_scope_digest"] == edit["evidence"]["prefix_scope_digest"],
            scope_matches
        );
        assert_eq!(
            work["evidence"]["cache_namespace_status"],
            if declare_namespace {
                "host_declared_unverified"
            } else {
                "unknown"
            }
        );
        assert_eq!(edit["evidence"]["physical_cache_hit"], "unknown");
        assert!(
            !work["evidence"]
                .to_string()
                .contains("fixture-declared-scope")
        );
    }
    server.abort();
}

#[tokio::test]
async fn rejected_worker_response_retains_reported_usage_without_adopting_the_answer() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(truncated_worker)),
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
    let created = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(created["result"]["chain_id"].as_str().unwrap().into());
    core.operate(
        "source.ingest",
        &context,
        &json!({"request":{"messages":[{"role":"user","content":"fixture"}]}}),
    )
    .await
    .unwrap();
    assert!(
        core.operate("worker.run", &context, &json!({}))
            .await
            .is_err()
    );
    let usage = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    let group = &usage["result"]["groups"][0];
    assert_eq!(group["role"], "worker");
    assert_eq!(group["prompt_tokens"]["known_tokens"], 11);
    assert_eq!(group["completion_tokens"]["known_tokens"], 9);
    assert_eq!(group["prompt_tokens"]["unknown_attempts"], 0);
    server.abort();
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
    drop(requests);
    let usage = core
        .operate("accounting.get", &context, &json!({}))
        .await
        .unwrap();
    let group = &usage["result"]["groups"][0];
    assert_eq!(group["attempts"], 2);
    assert_eq!(group["prompt_tokens"]["known_tokens"], 14);
    assert_eq!(group["completion_tokens"]["known_tokens"], 6);
    assert_eq!(group["prompt_tokens"]["unknown_attempts"], 0);
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
    let mut worker_profile = p.clone();
    worker_profile["model"] = json!("openai/worker-test-model");
    worker_profile["expected_returned_models"] = json!(["deepseek-v4.1-flash-eco"]);
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":worker_profile,"editor":p}}),
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
    let groups = usage["result"]["groups"].as_array().unwrap();
    let editor = groups.iter().find(|g| g["role"] == "editor").unwrap();
    let worker = groups.iter().find(|g| g["role"] == "worker").unwrap();
    assert_eq!(editor["requested_model"], "openai/deepseek-v4.1-flash-eco");
    assert_eq!(editor["attempts"], 2);
    assert_eq!(editor["prompt_tokens"]["known_tokens"], 20);
    assert_eq!(worker["requested_model"], "openai/worker-test-model");
    assert_eq!(worker["attempts"], 1);
    assert_eq!(worker["completion_tokens"]["known_tokens"], 5);
    assert_eq!(worker["cached_prompt_tokens"]["unknown_attempts"], 1);
    assert!(!usage.to_string().contains("TEST_MOCK_KEY"));
    server.abort();
}
