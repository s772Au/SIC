use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{path::Path, sync::Arc};
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Mock {
    requests: Arc<Mutex<Vec<Value>>>,
}

fn function_call(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}

async fn model(State(mock): State<Mock>, Json(request): Json<Value>) -> Json<Value> {
    let mut rows = mock.requests.lock().await;
    rows.push(request);
    let response = match rows.len() {
        1 => {
            json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[
                function_call("multi-alpha","sic_tool_call",json!({"name":"alpha","arguments":{"value":"a"}})),
                function_call("multi-beta","sic_tool_call",json!({"name":"beta","arguments":{"value":"b"}}))
            ]}}]})
        }
        2 => {
            json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[
                function_call("revoked-beta","sic_tool_call",json!({"name":"beta","arguments":{"value":"must-not-dispatch"}}))
            ]}}]})
        }
        _ => {
            json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"All host tasks were reconciled."}}]})
        }
    };
    Json(response)
}

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

fn profile(endpoint: &str) -> Value {
    json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":endpoint,"api_key_env":"LOCAL_FIXTURE_ONLY",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native",
        "context_window":100000,"max_input_tokens":100000,"max_output_tokens":4096,"default_output_tokens":512,"timeout_seconds":5})
}

fn core(store: Store, endpoint: &str) -> Core {
    let p = profile(endpoint);
    Core::with_model_config_and_resolver(
        store,
        json!({"roles":{"worker":p,"editor":p}}),
        Arc::new(|_| Ok("fixture-only".into())),
    )
    .unwrap()
}

async fn call(core: &Core, ctx: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, ctx, &payload)
        .await
        .unwrap_or_else(|e| panic!("{operation} failed: {e:?}"))["result"]
        .clone()
}

async fn create_store(path: &Path) -> Store {
    Store::create(path).await.unwrap()
}

fn grant(name: &str) -> Value {
    json!({"principal":"alice","tool_name":name})
}

fn descriptor(name: &str) -> Value {
    json!({"host_fixed":false,"descriptor":{"name":name,"description":format!("fixture {name}"),
        "schema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}})
}

fn configure() -> Value {
    let alpha = grant("alpha");
    let beta = grant("beta");
    json!({"acl":{"discover":[alpha.clone(),beta.clone()],"expand":[alpha.clone(),beta.clone()],"execute":[alpha,beta]},
        "tools":[descriptor("alpha"),descriptor("beta")]})
}

#[tokio::test]
async fn multiple_host_tool_leases_survive_partial_completion_restart_and_catalog_revocation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("multitool.sqlite");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mock = Mock::default();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(mock.clone()),
        )
        .into_future(),
    );
    let endpoint = format!("http://{address}/v1");

    let store = create_store(&db_path).await;
    let store_to_close = store.clone();
    let first_core = core(store, &endpoint);
    let initial = call(
        &first_core,
        &context("alice", None),
        "chain.create",
        json!({}),
    )
    .await;
    let chain_id = initial["chain_id"].as_str().unwrap().to_owned();
    let alice = context("alice", Some(&chain_id));
    call(&first_core, &alice, "tools.configure", configure()).await;
    call(
        &first_core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"run alpha and beta"}]}}),
    )
    .await;

    let response = call(&first_core, &alice, "worker.run", json!({})).await;
    let tasks = response["host_tasks"].as_array().unwrap();
    assert_eq!(
        tasks.len(),
        2,
        "both calls in one model response must become host leases"
    );
    let alpha_task = tasks
        .iter()
        .find(|task| task["descriptor"]["name"] == "alpha")
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let beta_task = tasks
        .iter()
        .find(|task| task["descriptor"]["name"] == "beta")
        .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        call(&first_core, &alice, "tools.status", json!({})).await["active_leases"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        first_core
            .operate("worker.run", &alice, &json!({}))
            .await
            .unwrap_err()
            .code,
        "tool_group_open"
    );
    assert_eq!(
        mock.requests.lock().await.len(),
        1,
        "open host calls must block another model dispatch"
    );

    first_core
        .operate(
            "tool.claim",
            &alice,
            &json!({"task_id":alpha_task,"claim_id":"alpha-claim"}),
        )
        .await
        .unwrap();
    let alpha_result = json!({"task_id":alpha_task,"claim_id":"alpha-claim","succeeded":true,"result":{"receipt":"alpha-accepted"}});
    let alpha_done = first_core
        .operate("tool.complete", &alice, &alpha_result)
        .await
        .unwrap();
    assert_eq!(alpha_done["result"]["status"], "completed");
    assert_eq!(
        alpha_done["result"]["open_calls"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        first_core
            .operate("tool.complete", &alice, &alpha_result)
            .await
            .unwrap()["evidence"]["replayed"],
        true
    );
    assert_eq!(
        first_core
            .operate(
                "tool.complete",
                &alice,
                &json!({"task_id":alpha_task,"claim_id":"alpha-claim","succeeded":false,"result":{"receipt":"different"}})
            )
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(
        first_core
            .operate("worker.run", &alice, &json!({}))
            .await
            .unwrap_err()
            .code,
        "tool_group_open"
    );
    let statuses = call(&first_core, &alice, "tools.tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        statuses
            .iter()
            .find(|task| task["task_id"] == alpha_task)
            .unwrap()["status"],
        "completed"
    );
    assert_eq!(
        statuses
            .iter()
            .find(|task| task["task_id"] == beta_task)
            .unwrap()["status"],
        "pending"
    );
    assert_eq!(
        call(&first_core, &alice, "tools.status", json!({})).await["active_leases"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        mock.requests.lock().await.len(),
        1,
        "partial completion cannot dispatch a duplicate or a fresh Worker call"
    );

    // Simulate process restart on the same durable DB and host profile. The open
    // beta lease and completed alpha receipt must both survive without a rerun.
    drop(first_core);
    store_to_close.close().await;
    let reopened = Store::open(&db_path).await.unwrap();
    let recovered_core = core(reopened, &endpoint);
    let recovered_tasks = call(&recovered_core, &alice, "tools.tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        recovered_tasks
            .iter()
            .find(|task| task["task_id"] == alpha_task)
            .unwrap()["status"],
        "completed"
    );
    assert_eq!(
        recovered_tasks
            .iter()
            .find(|task| task["task_id"] == beta_task)
            .unwrap()["status"],
        "pending"
    );
    assert_eq!(
        call(&recovered_core, &alice, "tools.status", json!({})).await["active_leases"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        recovered_core
            .operate("worker.run", &alice, &json!({}))
            .await
            .unwrap_err()
            .code,
        "tool_group_open"
    );
    assert_eq!(
        mock.requests.lock().await.len(),
        1,
        "restart must not redispatch either persisted lease"
    );
    assert_eq!(
        recovered_core
            .operate("tool.complete", &alice, &alpha_result)
            .await
            .unwrap()["evidence"]["replayed"],
        true
    );

    // While beta is still in flight, sync the host catalog to gamma and revoke
    // alpha/beta execution grants. Existing beta lease may finish exactly once;
    // the next model request naming beta must be rejected without a new lease.
    let gamma = grant("gamma");
    call(&recovered_core,&alice,"tools.sync",json!({"acl":{"discover":[gamma.clone()],"expand":[gamma.clone()],"execute":[gamma]},"tools":[descriptor("gamma")]})).await;
    let changed = call(&recovered_core, &alice, "tools.status", json!({})).await;
    assert_eq!(changed["catalog"].as_array().unwrap().len(), 1);
    assert_eq!(changed["catalog"][0]["name"], "gamma");
    assert_eq!(changed["active_leases"].as_array().unwrap().len(), 1);
    recovered_core
        .operate(
            "tool.claim",
            &alice,
            &json!({"task_id":beta_task,"claim_id":"beta-claim"}),
        )
        .await
        .unwrap();
    let beta_result = json!({"task_id":beta_task,"claim_id":"beta-claim","succeeded":true,"result":{"receipt":"beta-accepted"}});
    assert_eq!(
        call(
            &recovered_core,
            &alice,
            "tool.complete",
            beta_result.clone()
        )
        .await["status"],
        "completed"
    );
    assert_eq!(
        recovered_core
            .operate(
                "tool.complete",
                &alice,
                &json!({"task_id":beta_task,"claim_id":"beta-claim","succeeded":true,"result":{"receipt":"tampered"}})
            )
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    let revoked_call = call(&recovered_core, &alice, "worker.run", json!({})).await;
    assert!(revoked_call["host_tasks"].as_array().unwrap().is_empty());
    assert!(
        revoked_call["tool_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["result"]["code"] == "forbidden")
    );
    let final_answer = call(&recovered_core, &alice, "worker.run", json!({})).await;
    assert_eq!(final_answer["content"], "All host tasks were reconciled.");
    assert_eq!(
        call(&recovered_core, &alice, "tools.status", json!({})).await["active_leases"],
        json!([])
    );
    assert_eq!(mock.requests.lock().await.len(), 3);
    server.abort();
}
