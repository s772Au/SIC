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

async fn complete(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    calls.push(body);
    if calls.len() == 1 {
        Json(
            json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
                "role":"assistant","content":null,
                "tool_calls":[{"id":"edit-empty","type":"function","function":{
                    "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"
                }}]
            }}]}),
        )
    } else {
        Json(
            json!({"model":"mock","choices":[{"finish_reason":"stop","message":{
                "role":"assistant","content":"worker done"
            }}]}),
        )
    }
}
fn profile(endpoint: String) -> Value {
    json!({"model":"openai/mock","api_base":endpoint,"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":32000,"max_input_tokens":32000,
        "max_output_tokens":2048,"default_output_tokens":128,"timeout_seconds":5})
}
fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
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
async fn actual_worker_and_editor_requests_share_common_prefix_and_differ_only_in_i() {
    let mock = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn({
        let mock = mock.clone();
        async move {
            let _ = axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(complete))
                    .with_state(mock),
            )
            .await;
        }
    });
    let store = Store::memory().await.unwrap();
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"unit text"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "edit.run", json!({"job_id":job,"editor":"e1"})).await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":"pair-cb"}),
    )
    .await;
    let worker = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(worker["result"]["content"], "worker done");

    let calls = mock.calls.lock().await;
    assert_eq!(calls.len(), 2, "{calls:?}");
    let editor = &calls[0];
    let worker = &calls[1];
    assert_eq!(editor["model"], worker["model"]);
    assert_eq!(
        serde_json::to_vec(&editor["tools"]).unwrap(),
        serde_json::to_vec(&worker["tools"]).unwrap(),
        "common tool schema bytes must match"
    );
    let editor_messages = editor["messages"].as_array().unwrap();
    let worker_messages = worker["messages"].as_array().unwrap();
    // The shared teaching is the common message prefix. The Worker then
    // renders the frozen current CB before host activation; the Editor has no
    // prior CB in this job, so its activation follows the teaching and its new
    // WB material follows the activation.
    assert_eq!(editor_messages[0], worker_messages[0]);
    assert_eq!(worker_messages[1]["content"], "unit text");
    assert_ne!(editor_messages[1], worker_messages[1]);
    let activation_editor: Value =
        serde_json::from_str(editor_messages[1]["content"].as_str().unwrap()).unwrap();
    let activation_worker: Value =
        serde_json::from_str(worker_messages[2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(activation_editor["sic_host_activation"]["mode"], "editor");
    assert_eq!(activation_worker["sic_host_activation"]["mode"], "worker");
    assert_eq!(
        activation_editor["sic_host_activation"]["activated_tools"],
        json!(["sic_edit_blocks"])
    );
    assert!(
        !activation_worker["sic_host_activation"]["activated_tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "sic_edit_blocks")
    );
    server.abort();
}
