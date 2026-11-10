use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    scheduler::{EditorMode, SchedulerPolicy},
    storage::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone, Default)]
struct MockProvider {
    calls: Arc<Mutex<Vec<Value>>>,
    first_call: Arc<Notify>,
    gate_first: Option<Arc<Semaphore>>,
    fail: bool,
}

async fn complete(
    State(mock): State<MockProvider>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let worker_mode = request["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("WORKER MODE"))
        })
    });
    let call_number = {
        let mut calls = mock.calls.lock().await;
        calls.push(request);
        calls.len()
    };
    if call_number == 1 {
        mock.first_call.notify_one();
        if let Some(gate) = &mock.gate_first {
            gate.acquire().await.unwrap().forget();
        }
    }
    if mock.fail {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":{"message":"fixture provider outcome unknown"}})),
        );
    }
    if worker_mode {
        return (
            StatusCode::OK,
            Json(json!({
                "model":"deepseek-v4.1-flash-eco",
                "usage":{"prompt_tokens":12,"completion_tokens":5},
                "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Peer response from the mock Worker."}}]
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":12,"completion_tokens":5},
            "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
                "tool_calls":[{"id":format!("edit-{call_number}"),"type":"function","function":{
                    "name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"
                }}]}}]
        })),
    )
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

async fn call(core: &Core, ctx: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, ctx, &payload).await.unwrap()["result"].clone()
}

async fn make_chain(core: &Core, actor: &str) -> String {
    call(core, &context(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish_program(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> Value {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":["drop_empty"]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await
}

fn policy(max_level: u32, max_model_calls: usize, max_actions: usize) -> SchedulerPolicy {
    let mut level_batch_sizes = BTreeMap::new();
    for level in 2..=max_level {
        level_batch_sizes.insert(level, 2);
    }
    SchedulerPolicy {
        context_id: "main".into(),
        min_sources_per_wb: 1,
        max_sources_per_wb: 1,
        max_units_per_wb: 64,
        level_batch_sizes,
        max_level,
        flush: true,
        max_actions,
        max_model_calls,
        editor_mode: EditorMode::Provider,
        program_rules: Vec::new(),
    }
}

async fn setup(
    mock: MockProvider,
) -> (
    Core,
    OperationContext,
    String,
    String,
    tokio::task::JoinHandle<()>,
) {
    setup_with_lifecycle(mock, "P").await
}

async fn setup_with_lifecycle(
    mock: MockProvider,
    lifecycle: &str,
) -> (
    Core,
    OperationContext,
    String,
    String,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(complete))
                .with_state(mock),
        )
        .await
        .unwrap();
    });
    let profile = json!({
        "model":"openai/deepseek-v4.1-flash-eco",
        "api_base":format!("http://{address}/v1"),
        "api_key_env":"SIC_TEST_MOCK_KEY",
        "tokenizer_model":"host-o200k-surrogate",
        "tokenizer_status":"surrogate-not-provider-native",
        "context_window":1000000,
        "max_input_tokens":1000000,
        "max_output_tokens":65536,
        "default_output_tokens":4096,
        "timeout_seconds":10
    });
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let alice_chain = make_chain(&core, "alice").await;
    let bob_chain = make_chain(&core, "bob").await;
    let alice = context("alice", Some(&alice_chain));
    let published = publish_program(&core, &alice, "checkpoint source", "parent-cut").await;
    let checkpoint = call(
        &core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":published["cut_id"]}),
    )
    .await;
    let socket = call(
        &core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[],"signal":[],"spawn":["bob"],"read":[]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let parent = context("bob", Some(&bob_chain));
    let spawn = call(
        &core,
        &parent,
        "socket.spawn",
        json!({"socket_id":socket,"request_id":"prepare-task-1","message_id":"prepare-message-1",
              "text":"Inspect this checkpoint and prepare a concise report.","lifecycle":lifecycle}),
    )
    .await;
    let child = spawn["chain_id"].as_str().unwrap().to_owned();
    let task_id = spawn["task_id"].as_str().unwrap().to_owned();
    let child_context = context("bob", Some(&child));
    core.operate(
        "config.update",
        &child_context,
        &json!({"editor_feedback":{"max_corrections":0,"feedback_chars":8192}}),
    )
    .await
    .unwrap();
    let claim = call(
        &core,
        &parent,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":"prepare-host-claim"}),
    )
    .await;
    assert_eq!(claim["execution_permit"], true);
    (core, parent, task_id, child, server)
}

fn prepare_payload(task_id: &str, max_calls: usize, max_actions: usize) -> Value {
    json!({
        "task_id":task_id,
        "claim_id":"prepare-host-claim",
        "expected_revision":1,
        "prepare_id":"prepare-once-1",
        "policy":serde_json::to_value(policy(1,max_calls,max_actions)).unwrap()
    })
}

#[tokio::test]
async fn prepare_runs_real_editor_and_requires_this_spawn_message_in_eligible_cb() {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let (core, parent, task_id, child, server) = setup(mock).await;
    let payload = prepare_payload(&task_id, 2, 64);
    let result = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(result["result"]["state"], "prepared");
    assert_eq!(result["result"]["ready_for_dsh"], true);
    assert_eq!(result["result"]["peer_chain_id"], child);
    assert_eq!(result["result"]["evidence"]["provider_called"], true);
    assert_eq!(
        result["result"]["evidence"]["spawn_message_delivery"]["verified"],
        true
    );
    assert_eq!(
        result["result"]["evidence"]["spawn_message_delivery"]["content_retained"]["status"],
        "verified"
    );
    assert_eq!(result["result"]["evidence"]["worker_run"], false);
    assert_eq!(
        result["result"]["evidence"]["external_tools_executed"],
        false
    );
    assert_eq!(
        result["result"]["evidence"]["requester_message_delivered"],
        false
    );
    assert_eq!(calls.lock().await.len(), 1);

    let child_state = call(&core, &context("bob", Some(&child)), "cut.get", json!({})).await;
    assert!(child_state["view"]["cbs"].as_array().unwrap().len() >= 1);
    let repeated = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(repeated["result"]["replayed"], true);
    assert_eq!(repeated["result"]["execution_started"], false);
    assert_eq!(repeated["evidence"]["provider_called"], false);
    assert_eq!(calls.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn volatile_message_reaches_a_subsequent_editor_without_claiming_content_retention() {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let (core, requester, task_id, _peer, server) = setup_with_lifecycle(mock, "V").await;
    let result = core
        .operate(
            "peer.task.prepare",
            &requester,
            &prepare_payload(&task_id, 2, 64),
        )
        .await
        .unwrap();
    assert_eq!(result["result"]["state"], "prepared");
    assert_eq!(result["result"]["ready_for_dsh"], true);
    assert_eq!(
        result["result"]["evidence"]["spawn_message_delivery"]["verified"],
        true
    );
    assert_ne!(
        result["result"]["evidence"]["spawn_message_delivery"]["delivery_cb_id"],
        result["result"]["evidence"]["spawn_message_delivery"]["consume_cb_id"]
    );
    assert_eq!(
        result["result"]["evidence"]["spawn_message_delivery"]["content_retained"]["status"],
        "unknown"
    );
    assert_eq!(calls.lock().await.len(), 2);
    server.abort();
}

async fn create_peer_reply_socket(
    core: &Core,
    requester: &OperationContext,
    peer_chain: &str,
    allow_peer: bool,
) -> String {
    let published = publish_program(
        core,
        requester,
        "requester reply socket",
        "reply-socket-cut",
    )
    .await;
    let checkpoint = call(
        core,
        requester,
        "chain.checkpoint",
        json!({"cut_id":published["cut_id"]}),
    )
    .await;
    call(
        core,
        requester,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":if allow_peer {json!([peer_chain])} else {json!([])},
            "signal":[],"spawn":[],"read":[]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn peer_reply_delivers_only_latest_post_prepare_worker_result_once() {
    let (core, requester, task_id, peer_chain, server) = setup(MockProvider::default()).await;
    let prepared = core
        .operate(
            "peer.task.prepare",
            &requester,
            &prepare_payload(&task_id, 2, 64),
        )
        .await
        .unwrap();
    assert_eq!(prepared["result"]["ready_for_dsh"], true);
    let peer_context = context("bob", Some(&peer_chain));
    let worker = core
        .operate("worker.run", &peer_context, &json!({}))
        .await
        .unwrap();
    let attempt_id = worker["result"]["attempt_id"].as_str().unwrap();
    let socket_id = create_peer_reply_socket(&core, &requester, &peer_chain, true).await;

    let payload = json!({"task_id":task_id,"reply_id":"reply-once-1","expected_attempt_id":attempt_id,"socket_id":socket_id});
    let delivered = core
        .operate("peer.task.reply", &requester, &payload)
        .await
        .unwrap();
    assert_eq!(delivered["result"]["state"], "reply_delivered");
    assert_eq!(delivered["result"]["message_delivered"], true);
    assert_eq!(delivered["result"]["requester_adopted_cb"], false);
    assert_eq!(delivered["result"]["peer_chain_id"], peer_chain);
    assert_eq!(
        delivered["result"]["requester_chain_id"],
        requester.chain_id.clone().unwrap()
    );

    let inbox = call(&core, &requester, "socket.inbox", json!({})).await;
    let messages = inbox["messages"].as_array().unwrap();
    let reply_messages: Vec<_> = messages
        .iter()
        .filter(|entry| entry["message"]["phase_id"] == "peer-task-reply")
        .collect();
    assert_eq!(reply_messages.len(), 1);
    assert_eq!(
        reply_messages[0]["message"]["text"],
        "Peer response from the mock Worker."
    );
    assert!(reply_messages[0]["consumed_by_cut"].is_null());

    // A later Worker attempt cannot turn the already persisted receipt into a
    // second delivery permit; replay is resolved before latest-result checks.
    let later = core
        .operate("worker.run", &context("bob", Some(&peer_chain)), &json!({}))
        .await
        .unwrap();
    assert_ne!(later["result"]["attempt_id"], attempt_id);

    let replay = core
        .operate("peer.task.reply", &requester, &payload)
        .await
        .unwrap();
    assert_eq!(replay["evidence"]["replayed"], true);
    assert_eq!(replay["result"]["replayed"], true);
    let after_replay = call(&core, &requester, "socket.inbox", json!({})).await;
    assert_eq!(
        after_replay["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["message"]["phase_id"] == "peer-task-reply")
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn peer_reply_rejects_worker_output_made_stale_by_new_peer_input() {
    let (core, requester, task_id, peer_chain, server) = setup(MockProvider::default()).await;
    core.operate(
        "peer.task.prepare",
        &requester,
        &prepare_payload(&task_id, 2, 64),
    )
    .await
    .unwrap();
    let peer_context = context("bob", Some(&peer_chain));
    let worker = core
        .operate("worker.run", &peer_context, &json!({}))
        .await
        .unwrap();
    let attempt_id = worker["result"]["attempt_id"].as_str().unwrap();
    let socket_id = create_peer_reply_socket(&core, &requester, &peer_chain, true).await;
    call(
        &core,
        &peer_context,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"new input after Worker"}]}}),
    )
    .await;
    let denied = core
        .operate(
            "peer.task.reply",
            &requester,
            &json!({"task_id":task_id,"reply_id":"stale-reply","expected_attempt_id":attempt_id,"socket_id":socket_id}),
        )
        .await;
    assert!(denied.is_err());
    let inbox = call(&core, &requester, "socket.inbox", json!({})).await;
    assert!(
        !inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["message"]["phase_id"] == "peer-task-reply" })
    );
    server.abort();
}

#[tokio::test]
async fn peer_reply_requires_actor_authority_socket_acl_and_no_pending_tool() {
    let (core, requester, task_id, peer_chain, server) = setup(MockProvider::default()).await;
    core.operate(
        "peer.task.prepare",
        &requester,
        &prepare_payload(&task_id, 2, 64),
    )
    .await
    .unwrap();
    let peer_context = context("bob", Some(&peer_chain));
    let worker = core
        .operate("worker.run", &peer_context, &json!({}))
        .await
        .unwrap();
    let attempt_id = worker["result"]["attempt_id"].as_str().unwrap();
    let socket_id = create_peer_reply_socket(&core, &requester, &peer_chain, false).await;
    let payload = json!({"task_id":task_id,"reply_id":"reply-guarded-1","expected_attempt_id":attempt_id,"socket_id":socket_id});

    let charlie_route = make_chain(&core, "charlie").await;
    assert!(
        core.operate(
            "peer.task.reply",
            &context("charlie", Some(&charlie_route)),
            &payload
        )
        .await
        .is_err()
    );
    assert!(
        core.operate("peer.task.reply", &requester, &payload)
            .await
            .is_err()
    );

    call(
        &core,
        &requester,
        "socket.acl.update",
        json!({"socket_id":socket_id,"acl":{"message":[peer_chain],"signal":[],"spawn":[],"read":[]}}),
    )
    .await;

    // An explicitly unknown external outcome blocks replies until reconciled.
    let mut tx = core.store.begin().await.unwrap();
    sqlx::query("INSERT INTO host_tool_tasks(id,state) VALUES(?,?)")
        .bind("fixture-unresolved-peer-tool")
        .bind(json!({"task_id":"fixture-unresolved-peer-tool","chain_id":peer_chain,"actor":"bob","status":"unknown","call_id":"tool-call","source_id":"fixture-source","lease":{"id":"fixture-lease","tool_name":"fixture"}}).to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        core.operate("peer.task.reply", &requester, &payload)
            .await
            .is_err()
    );
    server.abort();
}

#[tokio::test]
async fn concurrent_prepare_replay_does_not_dispatch_a_second_provider_call() {
    let gate = Arc::new(Semaphore::new(0));
    let mock = MockProvider {
        gate_first: Some(gate.clone()),
        ..Default::default()
    };
    let calls = mock.calls.clone();
    let first_call = mock.first_call.clone();
    let (core, parent, task_id, _child, server) = setup(mock).await;
    let payload = prepare_payload(&task_id, 2, 64);
    let prepare_core = core.clone();
    let prepare_context = parent.clone();
    let prepare_payload = payload.clone();
    let first = tokio::spawn(async move {
        prepare_core
            .operate("peer.task.prepare", &prepare_context, &prepare_payload)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), first_call.notified())
        .await
        .unwrap();
    let replay = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(replay["result"]["state"], "running");
    assert_eq!(replay["result"]["replayed"], true);
    assert_eq!(replay["result"]["execution_started"], false);
    assert_eq!(calls.lock().await.len(), 1);
    gate.add_permits(1);
    assert_eq!(first.await.unwrap().unwrap()["result"]["state"], "prepared");
    assert_eq!(calls.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn provider_failure_is_quarantined_and_budget_rejection_does_not_call_provider() {
    let failing = MockProvider {
        fail: true,
        ..Default::default()
    };
    let fail_calls = failing.calls.clone();
    let (core, parent, task_id, _child, server) = setup(failing).await;
    let payload = prepare_payload(&task_id, 2, 64);
    let result = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(result["result"]["state"], "unknown");
    assert_eq!(result["result"]["ready_for_dsh"], false);
    assert_eq!(fail_calls.lock().await.len(), 1);
    let replay = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(replay["result"]["replayed"], true);
    assert_eq!(fail_calls.lock().await.len(), 1);
    server.abort();

    let mock = MockProvider::default();
    let budget_calls = mock.calls.clone();
    let (core, parent, task_id, child, server) = setup(mock).await;
    // With the default correction allowance, the scheduler reserves more than
    // one model attempt and refuses before contacting the fixture Provider.
    core.operate(
        "config.update",
        &context("bob", Some(&child)),
        &json!({"editor_feedback":{"max_corrections":2,"feedback_chars":8192}}),
    )
    .await
    .unwrap();
    let payload = prepare_payload(&task_id, 1, 64);
    let result = core
        .operate("peer.task.prepare", &parent, &payload)
        .await
        .unwrap();
    assert_eq!(result["result"]["state"], "unknown");
    assert_eq!(budget_calls.lock().await.len(), 0);
    server.abort();
}

#[tokio::test]
async fn prepare_rejects_unbounded_policy_before_claim_admission() {
    let mock = MockProvider::default();
    let calls = mock.calls.clone();
    let (core, parent, task_id, _child, server) = setup(mock).await;
    let payload = prepare_payload(&task_id, 9, 65);
    assert!(
        core.operate("peer.task.prepare", &parent, &payload)
            .await
            .is_err()
    );
    assert_eq!(calls.lock().await.len(), 0);
    let task = call(&core, &parent, "peer.tasks", json!({})).await;
    assert_eq!(task["tasks"][0]["state"], "claimed");
    server.abort();
}
