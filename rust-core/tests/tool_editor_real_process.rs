use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<usize>>,
}

async fn complete(State(mock): State<Mock>, Json(_body): Json<Value>) -> Json<Value> {
    let mut calls = mock.calls.lock().await;
    *calls += 1;
    if *calls == 1 {
        return Json(
            json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
                "role":"assistant","content":null,
                "tool_calls":[{"id":"host-call-1","type":"function","function":{
                    "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"
                }}]
            }}]}),
        );
    }
    drop(calls);
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    Json(
        json!({"model":"mock","usage":{"prompt_tokens":10,"completion_tokens":4},
        "choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,
            "tool_calls":[{"id":"edit-empty","type":"function","function":{
                "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"
            }}]
        }}]}),
    )
}
fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "alice".into(),
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
async fn editor_interval_overlaps_a_real_tool_process() {
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
    let profile = json!({"model":"openai/mock","api_base":format!("http://{address}/v1"),
        "api_key_env":"FIXTURE_ONLY","tokenizer_model":"host-surrogate",
        "context_window":32000,"max_input_tokens":32000,"max_output_tokens":2048,
        "default_output_tokens":128,"timeout_seconds":10});
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let grant = json!({"principal":"alice","tool_name":"weather"});
    call(
        &core,
        &ctx,
        "tools.configure",
        json!({"acl":{"discover":[grant.clone()],"expand":[grant.clone()],"execute":[grant]},
            "tools":[{"host_fixed":false,"descriptor":{"name":"weather","description":"weather lookup","schema":{
                "type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}]}),
    )
    .await;
    let edit_source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"editable material"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[edit_source]})).await;
    call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"weather"}]}}),
    )
    .await;
    call(&core, &ctx, "worker.run", json!({})).await;
    let task = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0].clone();
    let task_id = task["task_id"].as_str().unwrap().to_owned();
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":edit_source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Dispatch the Editor before the tool is claimed; the tool start therefore
    // falls inside the Editor interval.
    let editor_core = core.clone();
    let editor_ctx = ctx.clone();
    let editor_job = job.clone();
    let editor = tokio::spawn(async move {
        editor_core
            .operate(
                "edit.run",
                &editor_ctx,
                &json!({"job_id":editor_job,"editor":"e1"}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    call(
        &core,
        &ctx,
        "tool.claim",
        json!({"task_id":task_id,"claim_id":"host-claim"}),
    )
    .await;
    let claimed = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0]["claimed_at"]
        .as_u64()
        .unwrap();
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("tool-process.log");
    let command = format!(
        "Start-Sleep -Milliseconds 1600; Set-Content -LiteralPath '{}' -Value REAL-TOOL-DONE",
        log_path.display()
    );
    let status = tokio::task::spawn_blocking(move || {
        Command::new("pwsh")
            .args(["-NoProfile", "-Command", &command])
            .status()
            .expect("real tool process must start")
    })
    .await
    .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(&log_path).unwrap().trim(),
        "REAL-TOOL-DONE"
    );
    call(
        &core,
        &ctx,
        "tool.complete",
        json!({"task_id":task_id,"claim_id":"host-claim","succeeded":true,
               "result":{"temperature":"20C"}}),
    )
    .await;
    let editor_result = editor.await.unwrap().unwrap();
    assert_eq!(editor_result["result"]["status"], "ready");
    let completed = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0]["completed_at"]
        .as_u64()
        .unwrap();
    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let editor_interval = accounting["resources"]["timing"]["intervals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|interval| interval["role"] == "editor")
        .expect("editor interval");
    let (start, end) = (
        editor_interval["start"].as_u64().unwrap(),
        editor_interval["end"].as_u64().unwrap(),
    );
    let overlap = end.min(completed).saturating_sub(start.max(claimed));
    println!(
        "real-process intervals: editor={start}..{end} tool={claimed}..{completed} overlap={overlap}"
    );
    assert!(
        overlap >= 1,
        "editor {start}..{end} must overlap tool {claimed}..{completed}"
    );
    assert!(
        start <= claimed,
        "Editor must be dispatched before tool start"
    );
    assert!(
        claimed < end,
        "tool start must fall inside the Editor interval"
    );
    let tool_interval = accounting["resources"]["timing"]["intervals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|interval| interval["role"] == "tool")
        .expect("host tool interval");
    assert_eq!(tool_interval["start"], claimed);
    assert_eq!(tool_interval["end"], completed);
    assert!(
        accounting["resources"]["timing"]["overlap"]["tool_model"]
            .as_u64()
            .unwrap()
            >= 1,
        "accounting must expose the tool/model overlap"
    );
    let by_role = &accounting["resources"]["timing"]["by_role"];
    assert!(by_role["editor"].as_u64().unwrap() >= 1);
    assert!(by_role["tool"].as_u64().unwrap() >= 1);
    server.abort();
}
