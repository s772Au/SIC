use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockProvider {
    requests: Arc<Mutex<usize>>,
}

async fn complete(State(mock): State<MockProvider>, Json(_request): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    *requests += 1;
    Json(
        json!({"model":"mock","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,"tool_calls":[{"id":"host-call-1","type":"function","function":{
                "name":"sic_tool_call","arguments":"{\"name\":\"weather\",\"arguments\":{\"city\":\"Osaka\"}}"
            }}]
        }}]}),
    )
}

fn profile(endpoint: String) -> Value {
    json!({"model":"openai/mock","api_base":endpoint,"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate","context_window":32000,"max_input_tokens":32000,
        "max_output_tokens":2048,"default_output_tokens":128,"timeout_seconds":3})
}

fn context(chain_id: Option<String>) -> OperationContext {
    OperationContext {
        actor: "alice".into(),
        chain_id,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

async fn setup(path: &std::path::Path, endpoint: &str) -> (Store, Core, OperationContext, String) {
    let store = Store::create(path).await.unwrap();
    let p = profile(endpoint.to_owned());
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let owner = context(None);
    let chain = core
        .operate("chain.create", &owner, &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain.clone()));
    let grant = json!({"principal":"alice","tool_name":"weather"});
    core.operate("tools.configure", &ctx, &json!({"acl":{"discover":[grant.clone()],"expand":[grant.clone()],"execute":[grant]},
        "tools":[{"host_fixed":false,"descriptor":{"name":"weather","description":"weather lookup","schema":{
            "type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}]})).await.unwrap();
    core.operate(
        "source.ingest",
        &ctx,
        &json!({"request":{"messages":[{"role":"user","content":"weather"}]}}),
    )
    .await
    .unwrap();
    let worker = core.operate("worker.run", &ctx, &json!({})).await.unwrap();
    assert_eq!(worker["result"]["host_tasks"].as_array().unwrap().len(), 1);
    (store, core, ctx, chain)
}

#[tokio::test]
async fn task_claim_is_atomic_persistent_and_requires_matching_claim_to_complete() {
    let mock = MockProvider::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let dir = tempdir().unwrap();
    let path = dir.path().join("claims.sqlite");
    let (store, core, ctx, chain) = setup(&path, &format!("http://{address}/v1")).await;
    let tasks = core.operate("tools.tasks", &ctx, &json!({})).await.unwrap();
    let task_id = tasks["result"]["tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        core.operate(
            "tool.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"   "})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    assert_eq!(
        core.operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"x".repeat(129),"succeeded":null,"result":null})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    let left = core.clone();
    let contender_store = Store::open(&path).await.unwrap();
    let right = Core::new(contender_store.clone());
    let left_ctx = ctx.clone();
    let right_ctx = ctx.clone();
    let left_payload = json!({"task_id":task_id,"claim_id":"host-claim-a"});
    let right_payload = json!({"task_id":task_id,"claim_id":"host-claim-b"});
    let (left_result, right_result) = tokio::join!(
        left.operate("tool.claim", &left_ctx, &left_payload),
        right.operate("tool.claim", &right_ctx, &right_payload)
    );
    assert_ne!(
        left_result.is_ok(),
        right_result.is_ok(),
        "exactly one host may claim a task"
    );
    let claim_id = if left_result.is_ok() {
        "host-claim-a"
    } else {
        "host-claim-b"
    };
    let winner = if let Some(result) = left_result.as_ref().ok() {
        result
    } else {
        right_result.as_ref().ok().unwrap()
    };
    assert_eq!(winner["result"]["execution_permit"], true);
    let losing = if left_result.is_err() {
        left_result.unwrap_err()
    } else {
        right_result.unwrap_err()
    };
    assert_eq!(losing.code, "conflict");
    drop(right);
    drop(contender_store);

    let replay = core
        .operate(
            "tool.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":claim_id}),
        )
        .await
        .unwrap();
    assert_eq!(replay["evidence"]["replayed"], true);
    assert_eq!(replay["result"]["execution_permit"], false);
    let wrong_completion = core
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"wrong-claim","succeeded":true,"result":{"ok":1}}),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_completion.code, "conflict");

    drop(core);
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let p = profile(format!("http://{address}/v1"));
    let recovered = Core::with_model_config_and_resolver(
        reopened.clone(),
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let recovered_ctx = context(Some(chain));
    let recovered_tasks = recovered
        .operate("tools.tasks", &recovered_ctx, &json!({}))
        .await
        .unwrap();
    assert_eq!(recovered_tasks["result"]["tasks"][0]["status"], "claimed");
    let no_reclaim = recovered
        .operate(
            "tool.claim",
            &recovered_ctx,
            &json!({"task_id":task_id,"claim_id":"different-after-restart"}),
        )
        .await
        .unwrap_err();
    assert_eq!(no_reclaim.code, "conflict");
    let same_key = recovered
        .operate(
            "tool.claim",
            &recovered_ctx,
            &json!({"task_id":task_id,"claim_id":claim_id}),
        )
        .await
        .unwrap();
    assert_eq!(same_key["evidence"]["replayed"], true);
    assert_eq!(same_key["result"]["execution_permit"], false);

    let completion = json!({"task_id":task_id,"claim_id":claim_id,"succeeded":true,"result":{"external_receipt":"fixture-only"}});
    let done = recovered
        .operate("tool.complete", &recovered_ctx, &completion)
        .await
        .unwrap();
    assert_eq!(done["result"]["status"], "completed");
    assert_eq!(
        recovered
            .operate("tool.complete", &recovered_ctx, &completion)
            .await
            .unwrap()["evidence"]["replayed"],
        true
    );
    let terminal_claim = recovered
        .operate(
            "tool.claim",
            &recovered_ctx,
            &json!({"task_id":task_id,"claim_id":claim_id}),
        )
        .await
        .unwrap();
    assert_eq!(terminal_claim["evidence"]["replayed"], true);
    assert_eq!(terminal_claim["result"]["status"], "completed");
    assert_eq!(terminal_claim["result"]["execution_permit"], false);
    let changed_claim = recovered
        .operate(
            "tool.claim",
            &recovered_ctx,
            &json!({"task_id":task_id,"claim_id":"new-after-completion"}),
        )
        .await
        .unwrap_err();
    assert_eq!(changed_claim.code, "conflict");
    let changed = recovered.operate("tool.complete", &recovered_ctx, &json!({"task_id":task_id,"claim_id":claim_id,"succeeded":true,"result":{"external_receipt":"changed"}})).await.unwrap_err();
    assert_eq!(changed.code, "conflict");
    assert_eq!(
        *mock.requests.lock().await,
        1,
        "reopen and task claim must not make another model call"
    );
    server.abort();
}

#[tokio::test]
async fn unclaimed_task_only_accepts_explicit_unknown_outcome() {
    let mock = MockProvider::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(mock),
        )
        .into_future(),
    );
    let dir = tempdir().unwrap();
    let path = dir.path().join("unclaimed.sqlite");
    let (_store, core, ctx, _chain) = setup(&path, &format!("http://{address}/v1")).await;
    let task_id = core.operate("tools.tasks", &ctx, &json!({})).await.unwrap()["result"]["tasks"]
        [0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let forged_success = core
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"succeeded":true,"result":{"effect":"must not be accepted"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(forged_success.code, "conflict");
    let unknown = core
        .operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"succeeded":null,"result":{"outcome":"unknown"}}),
        )
        .await
        .unwrap();
    assert_eq!(unknown["result"]["status"], "unknown");
    assert_eq!(
        core.operate(
            "tool.complete",
            &ctx,
            &json!({"task_id":task_id,"succeeded":null,"result":{"outcome":"unknown"}})
        )
        .await
        .unwrap()["evidence"]["replayed"],
        true
    );
    server.abort();
}

#[tokio::test]
async fn received_host_output_is_not_a_success_attestation_and_never_grants_reexecution() {
    let mock = MockProvider::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(mock),
        )
        .into_future(),
    );
    let dir = tempdir().unwrap();
    let path = dir.path().join("delivered.sqlite");
    let (store, core, ctx, chain) = setup(&path, &format!("http://{address}/v1")).await;
    let task_id = core.operate("tools.tasks", &ctx, &json!({})).await.unwrap()["result"]["tasks"]
        [0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(core.operate("tool.complete", &ctx,
        &json!({"task_id":task_id,"succeeded":null,"result":"returned text","delivery":"received"})).await.is_err());
    core.operate(
        "tool.claim",
        &ctx,
        &json!({"task_id":task_id,"claim_id":"host-1"}),
    )
    .await
    .unwrap();
    for invalid in [
        json!({"task_id":task_id,"claim_id":"host-1","succeeded":null,"result":{"outcome":"unknown"},"delivery":"received"}),
        json!({"task_id":task_id,"claim_id":"host-1","succeeded":true,"result":"text","delivery":"received"}),
        json!({"task_id":task_id,"claim_id":"host-1","succeeded":null,"result":"text","delivery":"invented"}),
    ] {
        assert!(core.operate("tool.complete", &ctx, &invalid).await.is_err());
    }
    let payload = json!({"task_id":task_id,"claim_id":"host-1","succeeded":null,"result":"returned text","delivery":"received"});
    let completed = core.operate("tool.complete", &ctx, &payload).await.unwrap();
    assert_eq!(completed["result"]["status"], "delivered_unattested");
    assert!(completed["result"]["succeeded"].is_null());
    assert_eq!(
        core.operate("tool.complete", &ctx, &payload).await.unwrap()["evidence"]["replayed"],
        true
    );
    assert!(core.operate("tool.complete", &ctx,
        &json!({"task_id":task_id,"claim_id":"host-1","succeeded":null,"result":"returned text"})).await.is_err());
    drop(core);
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let core = Core::new(reopened);
    let ctx = context(Some(chain));
    let replay = core
        .operate(
            "tool.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"host-1"}),
        )
        .await
        .unwrap();
    assert_eq!(replay["result"]["status"], "delivered_unattested");
    assert_eq!(replay["result"]["execution_permit"], false);
    assert_eq!(
        core.operate("tool.complete", &ctx, &payload).await.unwrap()["evidence"]["replayed"],
        true
    );
    server.abort();
}
