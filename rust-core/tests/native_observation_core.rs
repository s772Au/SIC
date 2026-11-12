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
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn completion(State(fixture): State<Fixture>, Json(request): Json<Value>) -> Json<Value> {
    let editor = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("Return exactly one sic_edit_blocks"))
        });
    let mut requests = fixture.requests.lock().await;
    let worker_count = requests
        .iter()
        .filter(|item| item["fixture_role"] == "worker")
        .count();
    requests.push(json!({"fixture_role":if editor {"editor"} else {"worker"},"request":request}));
    let message = if editor {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"editor-1","type":"function",
            "function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]})
    } else if worker_count == 0 {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"host-call-1","type":"function",
            "function":{"name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"}}]})
    } else {
        json!({"role":"assistant","content":"continued after observation"})
    };
    let finish = if message.get("tool_calls").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    Json(json!({"model":"mock","choices":[{"finish_reason":finish,"message":message}]}))
}

#[tokio::test]
async fn late_host_result_enters_future_editor_and_worker_i_without_rewriting_completed_result() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock","api_base":format!("http://{address}/v1"),"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":32000,"max_input_tokens":32000,
        "max_output_tokens":2048,"default_output_tokens":128,"timeout_seconds":3});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let mut ctx = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &ctx, &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    ctx.chain_id = Some(chain);
    let grant = json!({"principal":"alice","tool_name":"weather"});
    core.operate("tools.configure", &ctx, &json!({"acl":{"discover":[grant.clone()],"expand":[grant.clone()],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"weather","description":"weather lookup","schema":{
        "type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"weather task"}]}}),
    )
    .await
    .unwrap();
    let worker = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let task = worker["result"]["host_tasks"][0]["task_id"]
        .as_str()
        .unwrap();
    core.operate(
        "tool.claim",
        &ctx,
        &json!({"task_id":task,"claim_id":"host-1"}),
    )
    .await
    .unwrap();
    let completion = core
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task,"claim_id":"host-1",
        "succeeded":null,"result":"unattested fixture output","delivery":"received"}),
        )
        .await
        .unwrap();
    let observation = json!({"task_id":task,"claim_id":"host-1","event_id":"dsh:late-fixture",
        "source":"dsh_tool_result","outcome":"error"});
    let mut bad_claim = observation.clone();
    bad_claim["claim_id"] = json!("other");
    assert!(
        core.operate("tool.observe", &ctx, &bad_claim)
            .await
            .is_err()
    );
    assert_eq!(
        core.operate("tool.observe", &ctx, &observation)
            .await
            .unwrap()["result"]["recorded"],
        true
    );
    assert_eq!(
        core.operate("tool.observe", &ctx, &observation)
            .await
            .unwrap()["evidence"]["replayed"],
        true
    );
    let mut conflict = observation.clone();
    conflict["outcome"] = json!("ok");
    assert_eq!(
        core.operate("tool.observe", &ctx, &conflict)
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let tasks = core.operate("tools.tasks", &ctx, &json!({})).await.unwrap();
    assert_eq!(
        tasks["result"]["tasks"][0]["status"],
        "delivered_unattested"
    );
    assert!(tasks["result"]["tasks"][0]["completion"]["succeeded"].is_null());

    let pending =
        core.operate("cut.get", &ctx, &json!({})).await.unwrap()["result"]["pending"].clone();
    let wb = core
        .operate("wb.form", &ctx, &json!({"source_ids":pending}))
        .await
        .unwrap();
    let job = core
        .operate(
            "edit.prepare",
            &ctx,
            &json!({"source_id":wb["result"]["source_id"]}),
        )
        .await
        .unwrap();
    core.operate("edit.run", &ctx, &json!({"job_id":job["result"]["job_id"]}))
        .await
        .unwrap();
    core.operate("cut.publish", &ctx, &json!({"job_id":job["result"]["job_id"],"expected_cut":0,"idempotency_key":"late-observation"})).await.unwrap();
    let cut = core.operate("cut.get", &ctx, &json!({})).await.unwrap();
    assert!(
        !cut["result"]["view"]["cbs"][0]["body"]
            .as_str()
            .unwrap()
            .contains("dsh:late-fixture")
    );
    assert_eq!(
        core.operate("worker.run", &ctx, &json!({})).await.unwrap()["result"]["content"],
        "continued after observation"
    );
    let requests = fixture.requests.lock().await;
    for role in ["editor", "worker"] {
        let request = requests
            .iter()
            .rev()
            .find(|request| request["fixture_role"] == role)
            .unwrap();
        let text = request["request"].to_string();
        assert!(text.contains("late_host_tool_observations"));
        assert!(text.contains("dsh:late-fixture"));
        assert!(text.contains("external or background task"));
    }
    assert_eq!(completion["result"]["status"], "delivered_unattested");
    server.abort();
}
