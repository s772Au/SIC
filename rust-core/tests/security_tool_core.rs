use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use sqlx::{
    Row,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{path::Path, sync::Arc};
use tempfile::tempdir;
use tokio::{sync::Mutex, task::JoinHandle};

#[derive(Clone)]
struct Fixture {
    response: Arc<Mutex<Value>>,
}

async fn completion(State(fixture): State<Fixture>, Json(_request): Json<Value>) -> Json<Value> {
    Json(fixture.response.lock().await.clone())
}

fn context(
    actor: &str,
    chain: Option<&str>,
    allow_model: bool,
    allow_revoke: bool,
) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model,
        allow_revoke,
    }
}

fn profile(endpoint: String) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"LOCAL_FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":1000000,"max_input_tokens":1000000,
        "max_output_tokens":10000,"default_output_tokens":4096,"timeout_seconds":15})
}

async fn fixture_core(response: Value, store: Store) -> (Core, JoinHandle<()>, Arc<Mutex<Value>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let response = Arc::new(Mutex::new(response));
    let app = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(Fixture {
            response: response.clone(),
        });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let p = profile(format!("http://{address}/v1"));
    let core = Core::with_model_config_and_resolver(
        store,
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    (core, server, response)
}

async fn call(core: &Core, ctx: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, ctx, &payload).await.unwrap()["result"].clone()
}

async fn create_chain(core: &Core, actor: &str) -> String {
    call(
        core,
        &context(actor, None, false, false),
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

async fn publish_base(core: &Core, ctx: &OperationContext, source_id: &str) -> Value {
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
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"base"}),
    )
    .await
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":serde_json::to_string(&arguments).unwrap()}})
}

fn tool_response(calls: Vec<Value>) -> Value {
    json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":calls}}]})
}

fn tool_config() -> Value {
    let grant = json!({"principal":"alice","tool_name":"fixture_action"});
    json!({"acl":{"discover":[grant.clone()],"expand":[grant.clone()],"execute":[grant]},"tools":[{
        "host_fixed":false,"descriptor":{"name":"fixture_action","description":"local test action",
            "schema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}
    }]})
}

async fn configure_tool(core: &Core, ctx: &OperationContext) {
    call(core, ctx, "tools.configure", tool_config()).await;
}

#[tokio::test]
async fn worker_cannot_mark_decision_item_completed_without_host_attestation() {
    let response = tool_response(vec![tool_call(
        "decision-1",
        "sic_decision_write",
        json!({"events":[{
            "kind":"complete","target":"work-1","text":"completed by model","evidence":["model assertion"]
        }]}),
    )]);
    let (core, server, _) = fixture_core(response, Store::memory().await.unwrap()).await;
    let chain = create_chain(&core, "alice").await;
    let ctx = context("alice", Some(&chain), true, false);
    ingest(&core, &ctx, "do the task").await;
    call(
        &core,
        &ctx,
        "decision.update",
        json!({"expected_revision":0,"events":[{
            "id":"work-1","kind":"work","text":"perform action","evidence":[]
        }]}),
    )
    .await;

    let _ = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    let decision = call(&core, &ctx, "decision.get", json!({})).await;
    assert_eq!(
        decision["decision"]["views"]["Working on"][0]["status"],
        "active"
    );
    assert!(
        decision["decision"]["views"]["Working on"][0]
            .get("completion")
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn worker_self_stop_prevents_later_external_tool_dispatch_and_closes_calls() {
    let response = tool_response(vec![
        tool_call(
            "stop-self",
            "sic_signal_send",
            json!({"target":"socket-self","signal":"stop"}),
        ),
        tool_call(
            "external-after-stop",
            "sic_tool_call",
            json!({"name":"fixture_action","arguments":{"value":"should-not-run"}}),
        ),
    ]);
    let (core, server, fixture_response) =
        fixture_core(response, Store::memory().await.unwrap()).await;
    let chain = create_chain(&core, "alice").await;
    let ctx = context("alice", Some(&chain), true, false);
    let source = ingest(&core, &ctx, "published base").await;
    publish_base(&core, &ctx, &source).await;
    let checkpoint = call(&core, &ctx, "chain.checkpoint", json!({})).await;
    let socket = call(
        &core,
        &ctx,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[],"signal":[chain],"spawn":[],"read":[]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture_response.lock().await["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
        json!({"target":socket,"signal":"stop"}).to_string().into();
    configure_tool(&core, &ctx).await;
    let _ = core.operate("worker.run", &ctx, &json!({})).await;

    assert_eq!(
        call(&core, &ctx, "chain.get", json!({})).await["run_state"],
        "stopped"
    );
    let tasks = call(&core, &ctx, "tools.tasks", json!({})).await;
    assert!(
        tasks["tasks"].as_array().unwrap().is_empty(),
        "external task was dispatched after self-stop"
    );
    let status = call(&core, &ctx, "tools.status", json!({})).await;
    assert!(status["active_leases"].as_array().unwrap().is_empty());
    server.abort();
}

async fn create_file_store(path: &Path) -> Store {
    Store::create(path).await.unwrap()
}

#[tokio::test]
async fn revoked_tool_source_allows_redacted_host_reconciliation_without_ingest() {
    let response = tool_response(vec![tool_call(
        "external-1",
        "sic_tool_call",
        json!({
            "name":"fixture_action","arguments":{"value":"private fixture argument"}
        }),
    )]);
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sic.sqlite");
    let (core, server, _) = fixture_core(response, create_file_store(&db_path).await).await;
    let chain = create_chain(&core, "alice").await;
    let ctx = context("alice", Some(&chain), true, true);
    let _source = ingest(&core, &ctx, "pending action").await;
    configure_tool(&core, &ctx).await;
    let worker = call(&core, &ctx, "worker.run", json!({})).await;
    let task_id = worker["host_tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let source = worker["host_tasks"][0]["source_id"]
        .as_str()
        .unwrap()
        .to_owned();

    call(
        &core,
        &ctx,
        "tool.claim",
        json!({"task_id":task_id,"claim_id":"revoked-task-claim"}),
    )
    .await;

    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":source,"reason":"test revocation"}),
    )
    .await;
    let reconciled = call(
        &core,
        &ctx,
        "tool.complete",
        json!({"task_id":task_id,"claim_id":"revoked-task-claim","succeeded":true,
        "result":{"external_receipt":"private fixture result"}}),
    )
    .await;
    assert_eq!(reconciled["status"], "reconciled_revoked");
    assert_eq!(reconciled["reconciled_only"], true);

    let tasks = call(&core, &ctx, "tools.tasks", json!({})).await;
    let task = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["task_id"] == task_id)
        .unwrap();
    assert_eq!(task["revoked"], true);
    assert_eq!(
        task.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["task_id", "status", "revoked"])
    );

    // Inspect only fixture-generated source rows to prove the reconciled result was not ingested.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .read_only(true)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    let rows = sqlx::query("SELECT data FROM objects WHERE kind='source'")
        .fetch_all(&pool)
        .await
        .unwrap();
    let texts: Vec<String> = rows
        .into_iter()
        .map(|row| row.try_get("data").unwrap())
        .collect();
    assert!(
        texts
            .iter()
            .all(|data| !data.contains("private fixture result"))
    );
    pool.close().await;
    server.abort();
}
