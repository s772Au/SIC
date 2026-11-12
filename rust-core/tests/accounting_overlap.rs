use axum::{Json, Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Barrier, Mutex};

async fn completion(
    State(state): State<Arc<(Barrier, Mutex<Vec<String>>)>>,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    state
        .1
        .lock()
        .await
        .push(String::from_utf8_lossy(&body).into_owned());
    let _ = tokio::time::timeout(Duration::from_secs(10), state.0.wait()).await;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    (
        StatusCode::OK,
        Json(json!({
            "model": "deepseek-v4.1-flash-eco",
            "usage": {"prompt_tokens": 11, "completion_tokens": 7},
            "choices": [{"finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": null,
                "tool_calls": [{"id": "edit-empty", "type": "function",
                    "function": {"name": "sic_edit_blocks", "arguments": "{\"ops\":[],\"pvi\":{}}"}}]
            }}]
        })),
    )
}

#[tokio::test]
async fn accounting_measures_concurrent_editor_intervals_and_overlap() {
    let state = Arc::new((Barrier::new(2), Mutex::new(Vec::new())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_state = state.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(server_state),
        )
        .await;
    });
    let profile = json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "api_base": format!("http://{address}/v1"),
        "api_key_env": "SIC_TEST_MOCK_KEY",
        "tokenizer_model": "host-o200k-surrogate",
        "tokenizer_status": "surrogate-not-provider-native",
        "context_window": 1_000_000,
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 65536,
        "default_output_tokens": 4096,
        "timeout_seconds": 20
    });
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let mut ctx = OperationContext {
        actor: "owner".into(),
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
    let source = core
        .operate(
            "source.ingest",
            &ctx,
            &json!({"request":{"messages":[
                {"role":"user","content":"alpha fact"},
                {"role":"user","content":"beta fact"}
            ]}}),
        )
        .await
        .unwrap()["result"]["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    core.operate("wb.form", &ctx, &json!({"source_ids":[source]}))
        .await
        .unwrap();
    let units: Vec<String> = core
        .operate("source.read", &ctx, &json!({"source_id":source}))
        .await
        .unwrap()["result"]["source"]["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
        .collect();
    let job = core
        .operate(
            "edit.prepare",
            &ctx,
            &json!({"source_id":source,"assignments":[
                {"editor":"e1","unit_ids":[units[0]]},
                {"editor":"e2","unit_ids":[units[1]]}
            ]}),
        )
        .await
        .unwrap()["result"]["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_payload = json!({"job_id":job,"editor":"e1"});
    let second_payload = json!({"job_id":job,"editor":"e2"});
    let (first, second) = tokio::join!(
        core.operate("edit.run", &ctx, &first_payload),
        core.operate("edit.run", &ctx, &second_payload)
    );
    first.unwrap();
    second.unwrap();
    let accounting = core
        .operate("accounting.get", &ctx, &json!({}))
        .await
        .unwrap()["result"]
        .clone();
    let timing = &accounting["resources"]["timing"];
    let intervals = timing["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 2, "{timing}");
    assert!(
        intervals
            .iter()
            .all(|interval| interval["role"] == "editor")
    );
    assert!(
        timing["overlap"]["editor_editor"].as_u64().unwrap() >= 1,
        "concurrent editors must overlap by at least one second: {timing}"
    );
    assert!(timing["total_model_seconds"].as_u64().unwrap() >= 1);
    assert!(timing["first_start"].is_u64());
    assert!(timing["last_end"].is_u64());
    server.abort();
}
