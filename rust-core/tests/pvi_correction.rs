use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockState {
    calls: Arc<Mutex<Vec<String>>>,
}

async fn completion(State(state): State<MockState>, body: String) -> (StatusCode, Json<Value>) {
    state.calls.lock().await.push(body.clone());
    if body.contains("EDITOR MODE") {
        return (
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
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "model": "deepseek-v4.1-flash-eco",
            "usage": {"prompt_tokens": 16, "completion_tokens": 8},
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "answer"}}]
        })),
    )
}

async fn fixture() -> (
    Core,
    OperationContext,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
    Store,
) {
    let state = MockState::default();
    let calls = state.calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(state),
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
        "timeout_seconds": 10
    });
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "owner".into(),
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
    (core, context, calls, server, store)
}

async fn stored_object(store: &Store, id: &str) -> (String, String) {
    let mut tx = store.begin().await.unwrap();
    let row = sqlx::query("SELECT data,digest FROM objects WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let data: String = row.try_get("data").unwrap();
    let digest: String = row.try_get("digest").unwrap();
    tx.commit().await.unwrap();
    (data, digest)
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}

async fn ingest(core: &Core, ctx: &OperationContext, text: &str) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish_program(
    core: &Core,
    ctx: &OperationContext,
    source: &str,
    pvi: Value,
    key: &str,
) -> String {
    let prepared = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[],"pvi":pvi}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn request_text(body: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    request["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message["content"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn later_pvi_correction_is_used_while_the_old_fact_record_stays() {
    let (core, ctx, calls, server, store) = fixture().await;
    let old_fact = ingest(&core, &ctx, "old fact").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[old_fact]})).await;
    let first_cb = publish_program(
        &core,
        &ctx,
        &old_fact,
        json!({"P":["old conclusion"],"I":{"status":"draft"}}),
        "old-cb",
    )
    .await;
    let first_body = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"][0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_stored = stored_object(&store, &first_cb).await;
    assert!(first_body.contains("old conclusion"));

    // A later correction uses a new P plus a V for the next block and I for
    // the current interpretation.
    let new_evidence = ingest(&core, &ctx, "new evidence").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[new_evidence]})).await;
    let second_cb = publish_program(
        &core,
        &ctx,
        &new_evidence,
        json!({
            "P":["correction: old conclusion superseded"],
            "V":["new conclusion"],
            "I":{"status":"corrected"}
        }),
        "correction-cb",
    )
    .await;
    assert_ne!(first_cb, second_cb);
    let view = call(&core, &ctx, "cut.get", json!({})).await;
    let cbs = view["view"]["cbs"].as_array().unwrap();
    assert_eq!(cbs.len(), 2);
    assert_eq!(
        cbs[0]["body"].as_str().unwrap(),
        first_body,
        "the old CB body must stay identical"
    );
    assert_eq!(
        first_stored,
        stored_object(&store, &first_cb).await,
        "the old CB stored data/digest must stay byte-identical"
    );
    assert!(
        cbs[1]["body"].as_str().unwrap().contains("correction:"),
        "the correction persists in a new CB"
    );
    assert_eq!(view["view"]["instant"]["status"], "corrected");
    let pending = view["pending"].as_array().unwrap();
    assert!(
        !pending.is_empty(),
        "V becomes the next block's pending source"
    );
    let v_source = pending[0].as_str().unwrap().to_owned();
    let v_text =
        call(&core, &ctx, "source.read", json!({"source_id":v_source})).await["source"]["units"][0]
            ["text"]
            .as_str()
            .unwrap()
            .to_owned();
    assert_eq!(v_text, "new conclusion");

    // The next Worker prompt carries the old CB, the correction, the current
    // I view, and the V material.
    let question = ingest(&core, &ctx, "what now?").await;
    let _ = question;
    let worker = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(worker["result"]["content"], "answer");
    let worker_text = request_text(calls.lock().await.first().expect("worker request captured"));
    assert!(
        worker_text.contains("old conclusion"),
        "old fact record remains"
    );
    assert!(worker_text.contains("correction: old conclusion superseded"));
    assert!(worker_text.contains("new conclusion"));
    assert!(worker_text.contains("\"status\":\"corrected\""));

    // The next Editor input uses the corrected I view while editing a new WB.
    let next_sources = call(&core, &ctx, "cut.get", json!({})).await["pending"]
        .as_array()
        .unwrap()
        .clone();
    let ids: Vec<Value> = next_sources.clone();
    call(&core, &ctx, "wb.form", json!({"source_ids":ids})).await;
    let source = next_sources[0].as_str().unwrap().to_owned();
    let prepared = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await;
    let job = prepared["job_id"].as_str().unwrap().to_owned();
    let editor = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap();
    assert_eq!(editor["result"]["status"], "ready");
    let calls = calls.lock().await;
    let editor_text = request_text(calls.last().expect("editor request captured"));
    assert!(
        editor_text.contains("\"status\":\"corrected\""),
        "{editor_text}"
    );
    assert!(editor_text.contains("old conclusion"));
    assert!(!editor_text.contains("\"status\":\"draft\""));
    server.abort();
}
