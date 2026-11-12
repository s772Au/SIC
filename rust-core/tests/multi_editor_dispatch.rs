use axum::{Json, Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Barrier, Mutex};

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
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 393_216,
        "default_output_tokens": 4096,
        "max_supported_output_tokens": 393_216,
        "context_window": 1_000_000,
        "timeout_seconds": 10
    })
}

#[derive(Clone)]
struct MockState {
    barrier: Arc<Barrier>,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Two Editors must be in flight before either response is produced. A build
/// that serialized the dispatch would leave the first request waiting and this
/// handler would answer 500 instead of the edit.
async fn completion(State(state): State<MockState>, body: Bytes) -> (StatusCode, Json<Value>) {
    let text = String::from_utf8_lossy(&body).into_owned();
    state.requests.lock().await.push(text.clone());
    if tokio::time::timeout(Duration::from_secs(5), state.barrier.wait())
        .await
        .is_err()
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Editor dispatch was serialized"})),
        );
    }
    let (target, content) = if text.contains("alpha fact") {
        ("u1", "alpha edited")
    } else if text.contains("beta fact") {
        ("u2", "beta risk")
    } else {
        ("u3", "unexpected unit")
    };
    let arguments =
        json!({"ops":[{"op":"REPLACE","target":target,"content":content}],"pvi":{}}).to_string();
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
                        "id": format!("edit-{target}"),
                        "type": "function",
                        "function": {"name": "sic_edit_blocks", "arguments": arguments}
                    }]
                }
            }]
        })),
    )
}

/// The trusted activation is canonical JSON text inside a message, so the raw
/// HTTP body escapes its quotes. Parse the envelope, then join the text.
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
fn request_mentions_editor(body: &str, editor: &str) -> bool {
    request_text(body).contains(&format!("\"editor\":\"{editor}\""))
}

async fn private_background(
    core: &Core,
    ctx: &OperationContext,
    editor: &str,
    text: &str,
) -> String {
    let name = format!("editor:{editor}");
    call(
        core,
        ctx,
        "hierarchy.context.create",
        json!({"context_id":name,"editor":editor}),
    )
    .await;
    let source = call(
        core,
        ctx,
        "hierarchy.context.ingest",
        json!({"context_id":name,"request":{"messages":[{"role":"user","content":text}]}}),
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
    let cb = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],
               "idempotency_key":format!("private-{editor}")}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        core,
        ctx,
        "hierarchy.select",
        json!({"context_id":name,"level":1,"roles":[format!("editor:{editor}")]}),
    )
    .await;
    cb
}

async fn setup(core: &Core) -> (String, Vec<String>, String) {
    let chain = call(core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let source = call(
        core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"user","content":"alpha fact"},
            {"role":"user","content":"beta fact"},
            {"role":"user","content":"gamma fact"}
        ]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let source_object =
        call(core, &ctx, "source.read", json!({"source_id":source})).await["source"].clone();
    let units: Vec<String> = source_object["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
        .collect();
    (chain, units, source)
}

#[tokio::test]
async fn two_editors_run_concurrently_with_the_same_shared_prefix() {
    let state = MockState {
        barrier: Arc::new(Barrier::new(2)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(state.clone()),
        )
        .into_future(),
    );
    let config = json!({"roles":{
        "worker": profile(format!("http://{addr}/v1")),
        "editor": profile(format!("http://{addr}/v1"))
    }});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        config,
        Arc::new(|_| Ok("fixture-only-key".into())),
    )
    .unwrap();
    let (chain, units, source) = setup(&core).await;
    let ctx = context("owner", Some(&chain));
    private_background(&core, &ctx, "e1", "private note one").await;
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

    let first_payload = json!({"job_id":job,"editor":"e1"});
    let second_payload = json!({"job_id":job,"editor":"e2"});
    let (first, second) = tokio::join!(
        core.operate("edit.run", &ctx, &first_payload),
        core.operate("edit.run", &ctx, &second_payload)
    );
    let first = first.unwrap_or_else(|error| panic!("e1 failed: {error:?}"));
    let second = second.unwrap_or_else(|error| panic!("e2 failed: {error:?}"));
    let requests = state.requests.lock().await;
    assert_eq!(
        requests.len(),
        2,
        "both Editors must reach the Provider, not one serialized call"
    );
    assert!(
        requests
            .iter()
            .any(|body| request_mentions_editor(body, "e1")),
        "e1's trusted I must name its own job/editor"
    );
    assert!(
        requests
            .iter()
            .any(|body| request_mentions_editor(body, "e2")),
        "e2's trusted I must name its own job/editor"
    );
    let e1_text = request_text(
        requests
            .iter()
            .find(|body| request_mentions_editor(body, "e1"))
            .unwrap(),
    );
    let e2_text = request_text(
        requests
            .iter()
            .find(|body| request_mentions_editor(body, "e2"))
            .unwrap(),
    );
    assert!(
        e1_text.contains("private note one")
            && e1_text.contains("editor_private_background")
            && e1_text.contains("\"scope\":\"private\""),
        "e1 must receive its selected private background as read-only scope"
    );
    assert!(
        !e2_text.contains("private note one"),
        "e2 must not see e1's private material"
    );
    assert_ne!(requests[0], requests[1], "each Editor has its own scope");
    drop(requests);
    assert_eq!(
        first["evidence"]["common_prefix_digest"], second["evidence"]["common_prefix_digest"],
        "the shared prefix must not contain the Editor assignment"
    );
    let statuses: Vec<&str> = [&first, &second]
        .iter()
        .map(|result| result["result"]["status"].as_str().unwrap())
        .collect();
    assert!(
        statuses.contains(&"open") && statuses.contains(&"ready"),
        "{statuses:?}"
    );
    let pending_total = first["result"]["pending_editors"].as_array().unwrap().len()
        + second["result"]["pending_editors"]
            .as_array()
            .unwrap()
            .len();
    assert_eq!(pending_total, 1, "exactly one sibling was still pending");

    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    assert!(cut["view"]["cbs"].as_array().unwrap().is_empty());
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],
               "idempotency_key":"dispatch-publish"}),
    )
    .await;
    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    let cb = cut["view"]["cbs"].as_array().unwrap()[0].clone();
    assert_eq!(cb["cb_id"], published["cb_id"]);
    let body = cb["body"].as_str().unwrap();
    assert!(body.contains("alpha edited"), "{body}");
    assert!(body.contains("beta risk"), "{body}");
    assert!(
        body.contains("gamma fact"),
        "unassigned unit must stay KEEP: {body}"
    );
    assert_eq!(cb["editors"].as_array().unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn an_editor_cannot_respond_twice_through_dispatch() {
    let core = Core::new(Store::memory().await.unwrap());
    let (chain, units, source) = setup(&core).await;
    let mut ctx = context("owner", Some(&chain));
    ctx.allow_model = true;
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
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job,"editor":"e1","ops":[],"pvi":{}}),
    )
    .await;
    let error = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap_err();
    assert!(error.message.contains("already responded"), "{error:?}");
    let error = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e3"}))
        .await
        .unwrap_err();
    assert_eq!(error.status, 403);
}
