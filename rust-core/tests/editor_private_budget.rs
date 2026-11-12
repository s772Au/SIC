use axum::{Json, Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}

fn profile(api_base: String) -> Value {
    json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "expected_returned_models": ["deepseek-v4.1-flash-eco"],
        "api_base": api_base,
        "api_key_env": "SIC_TEST_PROVIDER_KEY",
        "tokenizer_model": "host-counter-gpt4o",
        "tokenizer_status": "host-selected-surrogate-not-provider-attested",
        "context_window": 200_000,
        "max_input_tokens": 60_000,
        "max_output_tokens": 4096,
        "default_output_tokens": 4096,
        "max_supported_output_tokens": 8192,
        "timeout_seconds": 20
    })
}

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<String>>>,
}

async fn completion(State(state): State<MockState>, body: Bytes) -> (StatusCode, Json<Value>) {
    state
        .requests
        .lock()
        .await
        .push(String::from_utf8_lossy(&body).into_owned());
    (
        StatusCode::OK,
        Json(json!({
            "model": "deepseek-v4.1-flash-eco",
            "usage": {"prompt_tokens": 11, "completion_tokens": 7},
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "edit-empty",
                        "type": "function",
                        "function": {
                            "name": "sic_edit_blocks",
                            "arguments": "{\"ops\":[],\"pvi\":{}}"
                        }
                    }]
                }
            }]
        })),
    )
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

async fn start_mock() -> (String, MockState, tokio::task::JoinHandle<()>) {
    let state = MockState {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router_state = state.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(router_state),
        )
        .await;
    });
    (format!("http://{addr}/v1"), state, server)
}

async fn core_with(api_base: String) -> Core {
    Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{
            "worker": profile(api_base.clone()),
            "editor": profile(api_base)
        }}),
        Arc::new(|_| Ok("fixture-only-key".into())),
    )
    .unwrap()
}

/// One Editor-level private representation: a huge level-1 CB and, optionally,
/// a host-compressed level-2 CB that replaces it.
async fn private_representation(core: &Core, ctx: &OperationContext, compress: bool) -> String {
    let name = "editor:e1";
    call(
        core,
        ctx,
        "hierarchy.context.create",
        json!({"context_id":name,"editor":"e1"}),
    )
    .await;
    let huge = format!(
        "HUGE-PRIVATE-MARKER {}",
        "lorem ipsum dolor sit amet ".repeat(30_000)
    );
    let source = call(
        core,
        ctx,
        "hierarchy.context.ingest",
        json!({"context_id":name,"request":{"messages":[{"role":"user","content":huge}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        core,
        ctx,
        "wb.form",
        json!({"context_id":name,"source_ids":[source]}),
    )
    .await;
    let prepared = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    let cb1 = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],
               "idempotency_key":"private-base"}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    if compress {
        let higher = call(
            core,
            ctx,
            "hierarchy.form",
            json!({"context_id":name,"level":2,"cb_ids":[cb1]}),
        )
        .await;
        let source2 = higher["source_id"].as_str().unwrap().to_owned();
        let unit =
            call(core, ctx, "source.read", json!({"source_id":source2})).await["source"]["units"]
                [0]["unit_id"]
                .as_str()
                .unwrap()
                .to_owned();
        let job = call(
            core,
            ctx,
            "edit.prepare",
            json!({"source_id":source2,"assignments":[{"editor":"e1","unit_ids":[unit]}]}),
        )
        .await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            core,
            ctx,
            "edit.submit",
            json!({"job_id":job,"editor":"e1",
                   "ops":[{"op":"REPLACE","target":unit,"content":"private summary"}],
                   "pvi":{}}),
        )
        .await;
        let state = call(core, ctx, "chain.get", json!({})).await;
        call(
            core,
            ctx,
            "cut.publish",
            json!({"job_id":job,"expected_cut":state["cut_seq"],
                   "idempotency_key":"private-compress"}),
        )
        .await;
    }
    call(
        core,
        ctx,
        "hierarchy.select",
        json!({"context_id":name,"level":1,"roles":["editor:e1"]}),
    )
    .await;
    cb1
}

async fn main_job(core: &Core, ctx: &OperationContext) -> String {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"main task"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let unit = call(core, ctx, "source.read", json!({"source_id":source})).await["source"]["units"]
        [0]["unit_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        core,
        ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":[unit]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn over_budget_private_background_falls_back_to_a_higher_formed_level() {
    let (base, state, server) = start_mock().await;
    let core = core_with(base).await;
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    private_representation(&core, &ctx, true).await;
    let job = main_job(&core, &ctx).await;
    let result = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap()["result"]
        .clone();
    assert_eq!(result["status"], "ready");
    let requests = state.requests.lock().await;
    assert_eq!(requests.len(), 1, "one Editor call");
    let text = request_text(&requests[0]);
    assert!(text.contains("private summary"), "level-2 CB must be used");
    assert!(
        text.contains("\"level\":2") && text.contains("\"fallback\":true"),
        "the fallback must be explicit in the trusted I"
    );
    assert!(
        !text.contains("HUGE-PRIVATE-MARKER"),
        "over-budget material must not be truncated into the request"
    );
    server.abort();
}

#[tokio::test]
async fn private_background_that_cannot_fit_is_refused_before_the_provider() {
    let (base, state, server) = start_mock().await;
    let core = core_with(base).await;
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    private_representation(&core, &ctx, false).await;
    let job = main_job(&core, &ctx).await;
    let error = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "editor_context_over_budget");
    assert!(
        state.requests.lock().await.is_empty(),
        "the Provider must not be called for a request that cannot fit"
    );
    server.abort();
}
