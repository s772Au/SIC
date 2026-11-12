use axum::{Json, Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

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
        "context_window": 1_000_000,
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 393_216,
        "default_output_tokens": 4096,
        "max_supported_output_tokens": 393_216,
        "timeout_seconds": 20
    })
}

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<String>>>,
    release: Arc<Semaphore>,
}

async fn completion(State(state): State<MockState>, body: Bytes) -> (StatusCode, Json<Value>) {
    state
        .requests
        .lock()
        .await
        .push(String::from_utf8_lossy(&body).into_owned());
    // Each dispatched Editor blocks until the test releases it, so claims and
    // in-flight accounting can be observed deterministically.
    let _permit = state.release.acquire().await.unwrap();
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
                        "function": {"name": "sic_edit_blocks", "arguments": "{\"ops\":[],\"pvi\":{}}"}
                    }]
                }
            }]
        })),
    )
}

async fn start_mock() -> (String, MockState, tokio::task::JoinHandle<()>) {
    let state = MockState {
        requests: Arc::new(Mutex::new(Vec::new())),
        release: Arc::new(Semaphore::new(0)),
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

async fn five_unit_job(core: &Core, ctx: &OperationContext) -> String {
    let messages: Vec<Value> = (1..=5)
        .map(|n| json!({"role":"user","content":format!("fact {n}")}))
        .collect();
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":messages}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let units: Vec<String> =
        call(core, ctx, "source.read", json!({"source_id":source})).await["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
            .collect();
    assert_eq!(units.len(), 5);
    let assignments: Vec<Value> = units
        .iter()
        .enumerate()
        .map(|(index, unit)| json!({"editor":format!("e{}", index + 1),"unit_ids":[unit]}))
        .collect();
    call(
        core,
        ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":assignments}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn at_most_four_editors_run_and_the_fifth_is_refused_before_the_provider() {
    let (base, state, server) = start_mock().await;
    let core = Arc::new(core_with(base).await);
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let job = five_unit_job(&core, &ctx).await;
    let mut tasks = Vec::new();
    for n in 1..=4 {
        let core = core.clone();
        let ctx = ctx.clone();
        let job = job.clone();
        tasks.push(tokio::spawn(async move {
            core.operate(
                "edit.run",
                &ctx,
                &json!({"job_id":job,"editor":format!("e{n}")}),
            )
            .await
        }));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while state.requests.lock().await.len() < 4 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "four Editors did not reach the Provider"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let refused = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e5"}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "editor_concurrency_limit");
    assert_eq!(
        state.requests.lock().await.len(),
        4,
        "the refused Editor must not reach the Provider"
    );
    state.release.add_permits(4);
    for task in tasks {
        task.await
            .unwrap()
            .unwrap_or_else(|error| panic!("Editor failed: {error:?}"));
    }
    // Claims are gone once the four responses are durable; e5 can run now.
    let last = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e5"}))
        .await
        .unwrap()["result"]
        .clone();
    assert_eq!(last["status"], "ready");
    let state_seq = call(&core, &ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state_seq["cut_seq"],
               "idempotency_key":"five-editors"}),
    )
    .await;
    let body = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"][0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(published["cb_id"].as_str().unwrap().len() > 0, true);
    for n in 1..=5 {
        assert!(body.contains(&format!("fact {n}")), "{body}");
    }
    server.abort();
}

#[tokio::test]
async fn abandoning_one_editor_fails_the_job_and_keeps_private_scope_intact() {
    let (base, state, server) = start_mock().await;
    let core = core_with(base).await;
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    // e1 owns a private representation; it must survive the failed job.
    let name = "editor:e1";
    call(
        &core,
        &ctx,
        "hierarchy.context.create",
        json!({"context_id":name,"editor":"e1"}),
    )
    .await;
    let private_source = call(
        &core,
        &ctx,
        "hierarchy.context.ingest",
        json!({"context_id":name,"request":{"messages":[{"role":"user","content":"private note one"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "wb.form",
        json!({"context_id":name,"source_ids":[private_source]}),
    )
    .await;
    let prepared = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":private_source,"rules":[]}),
    )
    .await;
    let state_seq = call(&core, &ctx, "chain.get", json!({})).await;
    call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state_seq["cut_seq"],
               "idempotency_key":"private-one"}),
    )
    .await;
    call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"context_id":name,"level":1,"roles":["editor:e1"]}),
    )
    .await;
    // Two-Editor job on the shared chain.
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"user","content":"alpha fact"},
            {"role":"user","content":"beta fact"}
        ]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let units: Vec<String> =
        call(&core, &ctx, "source.read", json!({"source_id":source})).await["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
            .collect();
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[
            {"editor":"e1","unit_ids":[units[0]]},
            {"editor":"e2","unit_ids":[units[1]]}
        ]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let core = Arc::new(core);
    let run_core = core.clone();
    let run_ctx = ctx.clone();
    let run_job = job.clone();
    let handle = tokio::spawn(async move {
        run_core
            .operate(
                "edit.run",
                &run_ctx,
                &json!({"job_id":run_job,"editor":"e1"}),
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while state.requests.lock().await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "e1 did not reach the Provider"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let abandoned = call(
        &core,
        &ctx,
        "runtime.abandon",
        json!({"kind":"editor","job_id":job,"editor":"e1"}),
    )
    .await;
    assert_eq!(abandoned["abandoned"], true);
    state.release.add_permits(1);
    let error = handle.await.unwrap().unwrap_err();
    assert!(
        error.message.contains("claim changed"),
        "a late response must not be adopted: {error:?}"
    );
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["jobs"][&job]["status"], "failed");
    assert!(
        state["jobs"][&job]["responses"]
            .as_object()
            .is_none_or(|responses| responses.is_empty()),
        "no Editor response may survive a failed job: {}",
        state["jobs"][&job]
    );
    let view = call(&core, &ctx, "hierarchy.get", json!({})).await;
    assert_eq!(view["role_views"]["editor:e1"]["context_id"], name);
    assert_eq!(view["role_views"]["editor:e1"]["level"], 1);
    assert_eq!(view["contexts"][name]["owner_editor"], "e1");
    let sibling = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e2"}))
        .await
        .unwrap_err();
    assert_eq!(sibling.code, "conflict");
    server.abort();
}
