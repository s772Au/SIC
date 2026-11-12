use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::process::Command;
use std::sync::Arc;
use tempfile::tempdir;

async fn complete(Json(_body): Json<Value>) -> Json<Value> {
    Json(
        json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,
            "tool_calls":[{"id":"host-call-1","type":"function","function":{
                "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"
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
async fn claimed_tool_survives_core_restart_while_external_process_runs() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(complete)),
        )
        .await;
    });
    let dir = tempdir().unwrap();
    let path = dir.path().join("tool-restart.sqlite");
    let store = Store::create_with_connections(&path, 4).await.unwrap();
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
    call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"weather"}]}}),
    )
    .await;
    call(&core, &ctx, "worker.run", json!({})).await;
    let task_id = call(&core, &ctx, "tools.tasks", json!({})).await["tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "tool.claim",
        json!({"task_id":task_id,"claim_id":"restart-claim"}),
    )
    .await;
    let log_path = dir.path().join("restart-tool.log");
    let command = format!(
        "Start-Sleep -Milliseconds 1200; Set-Content -LiteralPath '{}' -Value RESTART-TOOL-DONE",
        log_path.display()
    );
    let process = tokio::task::spawn_blocking(move || {
        Command::new("pwsh")
            .args(["-NoProfile", "-Command", &command])
            .status()
            .expect("restart tool process must start")
    });
    drop(core);
    store.close().await;
    let store = Store::open_with_connections(&path, 4).await.unwrap();
    let restarted = Core::new(store.clone());
    let before = call(&restarted, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(before["tasks"][0]["task_id"], task_id);
    assert_eq!(before["tasks"][0]["status"], "claimed");
    let second_claim = restarted
        .operate(
            "tool.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"second-claim"}),
        )
        .await
        .unwrap_err();
    assert_eq!(second_claim.code, "conflict");
    let wrong_claim = restarted
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"wrong-claim","succeeded":true,
                    "result":{"temperature":"20C"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_claim.code, "conflict");
    assert!(process.await.unwrap().success());
    assert_eq!(
        std::fs::read_to_string(&log_path).unwrap().trim(),
        "RESTART-TOOL-DONE"
    );
    let completed = call(
        &restarted,
        &ctx,
        "tool.complete",
        json!({"task_id":task_id,"claim_id":"restart-claim","succeeded":true,
               "result":{"temperature":"20C"}}),
    )
    .await;
    assert_eq!(completed["status"], "completed");
    let after = call(&restarted, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(after["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(after["tasks"][0]["status"], "completed");
    let replay = call(
        &restarted,
        &ctx,
        "tool.complete",
        json!({"task_id":task_id,"claim_id":"restart-claim","succeeded":true,
               "result":{"temperature":"20C"}}),
    )
    .await;
    assert_eq!(replay["status"], "completed");
    let after_replay = call(&restarted, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(after_replay["tasks"].as_array().unwrap().len(), 1);
    let tampered = restarted
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"restart-claim","succeeded":true,
                    "result":{"temperature":"different"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(tampered.code, "conflict");
    let after_tamper = call(&restarted, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(after_tamper["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(after_tamper["tasks"][0]["status"], "completed");
    store.close().await;
    let reopened = Store::open_with_connections(&path, 4).await.unwrap();
    let again = Core::new(reopened.clone());
    let durable = call(&again, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(durable["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(durable["tasks"][0]["status"], "completed");
    reopened.close().await;
    let third = Store::open_with_connections(&path, 4).await.unwrap();
    let core_again = Core::new(third.clone());
    let durable_again = call(&core_again, &ctx, "tools.tasks", json!({})).await;
    assert_eq!(durable_again["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(durable_again["tasks"][0]["status"], "completed");
    third.close().await;
    server.abort();
}
