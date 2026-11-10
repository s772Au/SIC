use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify};

fn context(actor: &str, chain: Option<&str>, allow_model: bool) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model,
        allow_revoke: false,
    }
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}

async fn chain(core: &Core, actor: &str) -> String {
    call(
        core,
        &context(actor, None, false),
        "chain.create",
        json!({}),
    )
    .await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
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

async fn publish(core: &Core, ctx: &OperationContext, source_id: &str, key: &str) -> Value {
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source_id,"rules":[]}),
    )
    .await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":key}),
    )
    .await
}

async fn control_peer(core: &Core, owner: &OperationContext) -> (OperationContext, String) {
    let b = chain(core, "bob").await;
    let bob = context("bob", Some(&b), false);
    let checkpoint = call(core, owner, "chain.checkpoint", json!({})).await;
    let socket = call(
        core,
        owner,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[b],"signal":[b],"spawn":[b],"read":[b]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (bob, socket)
}

#[tokio::test]
async fn replaying_old_pause_after_stop_keeps_route_stopped() {
    let core = Core::new(Store::memory().await.unwrap());
    let a = chain(&core, "alice").await;
    let alice = context("alice", Some(&a), false);
    let source = ingest(&core, &alice, "published base").await;
    publish(&core, &alice, &source, "base").await;
    let (bob, socket_id) = control_peer(&core, &alice).await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket_id,"signal_id":"pause-1","action":"pause"}),
    )
    .await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket_id,"signal_id":"stop-1","action":"stop"}),
    )
    .await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket_id,"signal_id":"pause-1","action":"pause"}),
    )
    .await;
    assert_eq!(
        call(&core, &alice, "chain.get", json!({})).await["run_state"],
        "stopped"
    );
}

#[derive(Clone, Default)]
struct Gate {
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn gated_completion(State(gate): State<Gate>, Json(body): Json<Value>) -> Json<Value> {
    let n = gate.calls.fetch_add(1, Ordering::SeqCst) + 1;
    gate.requests.lock().await.push(body);
    if n == 1 {
        gate.started.notify_one();
        gate.release.notified().await;
    }
    let message = if n == 1 {
        "late first result"
    } else {
        "recovered result"
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":message}}]}),
    )
}

async fn gated_editor_completion(State(gate): State<Gate>, Json(body): Json<Value>) -> Json<Value> {
    let n = gate.calls.fetch_add(1, Ordering::SeqCst) + 1;
    gate.requests.lock().await.push(body);
    if n == 1 {
        gate.started.notify_one();
        gate.release.notified().await;
    }
    let arguments = json!({"ops":[],"pvi":{"V":["late editor result"]}}).to_string();
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"editor-late-1","type":"function","function":{"name":"sic_edit_blocks","arguments":arguments}}]}}]}),
    )
}

fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":1000000,"max_input_tokens":1000000,
        "max_output_tokens":10000,"default_output_tokens":4096,"timeout_seconds":20})
}

async fn fixture_core(gate: Gate, editor: bool) -> (Core, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = if editor {
        Router::new()
            .route("/v1/chat/completions", post(gated_editor_completion))
            .with_state(gate)
    } else {
        Router::new()
            .route("/v1/chat/completions", post(gated_completion))
            .with_state(gate)
    };
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    (core, server)
}

#[tokio::test]
async fn stopped_worker_does_not_adopt_its_late_provider_response() {
    let gate = Gate::default();
    let (core, server) = fixture_core(gate.clone(), false).await;
    let a = chain(&core, "alice").await;
    let alice = context("alice", Some(&a), true);
    let source = ingest(&core, &alice, "published base").await;
    publish(&core, &alice, &source, "base").await;
    let (bob, socket_id) = control_peer(&core, &alice).await;

    let running_core = core.clone();
    let running_context = alice.clone();
    let pending = tokio::spawn(async move {
        running_core
            .operate("worker.run", &running_context, &json!({}))
            .await
    });
    gate.started.notified().await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket_id,"signal_id":"stop-worker","action":"stop"}),
    )
    .await;
    gate.release.notify_one();
    let _ = pending.await.unwrap();

    assert_eq!(
        call(&core, &alice, "chain.get", json!({})).await["run_state"],
        "stopped"
    );
    let view = call(&core, &alice, "cut.get", json!({})).await;
    assert!(
        view["pending"].as_array().unwrap().is_empty(),
        "late Worker response was ingested"
    );
    server.abort();
}

#[tokio::test]
async fn worker_result_is_rejected_when_new_source_arrives_during_call() {
    let gate = Gate::default();
    let (core, server) = fixture_core(gate.clone(), false).await;
    let a = chain(&core, "alice").await;
    let alice = context("alice", Some(&a), true);
    let base = ingest(&core, &alice, "published base").await;
    publish(&core, &alice, &base, "base").await;

    let running_core = core.clone();
    let running_context = alice.clone();
    let pending = tokio::spawn(async move {
        running_core
            .operate("worker.run", &running_context, &json!({}))
            .await
    });
    gate.started.notified().await;
    let fresh = ingest(&core, &alice, "arrived while Worker was running").await;
    gate.release.notify_one();
    let result = pending.await.unwrap();
    assert!(
        result.is_err(),
        "Worker response based on the old input snapshot was accepted"
    );

    let view = call(&core, &alice, "cut.get", json!({})).await;
    let pending_ids = view["pending"].as_array().unwrap();
    assert_eq!(pending_ids, &[json!(fresh)]);
    let saved = call(&core, &alice, "source.read", json!({"source_id":fresh})).await;
    assert!(
        saved["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|unit| unit["text"] == "arrived while Worker was running")
    );
    assert!(
        saved["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|unit| unit["text"] != "late first result")
    );
    server.abort();
}

#[tokio::test]
async fn paused_editor_does_not_adopt_its_late_candidate() {
    let gate = Gate::default();
    let (core, server) = fixture_core(gate.clone(), true).await;
    let a = chain(&core, "alice").await;
    let alice = context("alice", Some(&a), true);
    let source = ingest(&core, &alice, "published base").await;
    publish(&core, &alice, &source, "base").await;
    let fresh = ingest(&core, &alice, "editor target").await;
    let job = call(&core, &alice, "edit.prepare", json!({"source_id":fresh})).await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (bob, socket_id) = control_peer(&core, &alice).await;

    let running_core = core.clone();
    let running_context = alice.clone();
    let running_job = job.clone();
    let pending = tokio::spawn(async move {
        running_core
            .operate("edit.run", &running_context, &json!({"job_id":running_job}))
            .await
    });
    gate.started.notified().await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket_id,"signal_id":"pause-editor","action":"pause"}),
    )
    .await;
    gate.release.notify_one();
    let _ = pending.await.unwrap();

    assert_eq!(
        call(&core, &alice, "chain.get", json!({})).await["run_state"],
        "waiting"
    );
    let candidate = call(&core, &alice, "edit.candidate.get", json!({"job_id":job})).await;
    assert!(
        candidate["candidate"].is_null(),
        "late Editor response became a candidate"
    );
    server.abort();
}

#[tokio::test]
async fn host_abandon_allows_new_worker_and_rejects_old_claim_result() {
    let gate = Gate::default();
    let (core, server) = fixture_core(gate.clone(), false).await;
    let a = chain(&core, "alice").await;
    let alice = context("alice", Some(&a), true);
    let source = ingest(&core, &alice, "published base").await;
    publish(&core, &alice, &source, "base").await;

    let running_core = core.clone();
    let running_context = alice.clone();
    let old = tokio::spawn(async move {
        running_core
            .operate("worker.run", &running_context, &json!({}))
            .await
    });
    gate.started.notified().await;

    let abandoned = call(&core, &alice, "runtime.abandon", json!({"kind":"worker"})).await;
    assert!(abandoned["abandoned_claim_id"].is_string());
    let replacement = call(&core, &alice, "worker.run", json!({})).await;
    assert_eq!(replacement["content"], "recovered result");
    gate.release.notify_one();
    assert!(
        old.await.unwrap().is_err(),
        "old claim result was accepted after recovery"
    );

    let view = call(&core, &alice, "cut.get", json!({})).await;
    let pending_ids = view["pending"].as_array().unwrap();
    assert_eq!(pending_ids.len(), 1);
    let result_source = call(
        &core,
        &alice,
        "source.read",
        json!({"source_id":pending_ids[0]}),
    )
    .await;
    assert!(
        result_source["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|unit| unit["text"] == "recovered result")
    );
    assert!(
        result_source["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|unit| unit["text"] != "late first result")
    );
    server.abort();
}
