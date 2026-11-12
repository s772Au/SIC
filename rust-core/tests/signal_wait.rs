use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Notify};

fn context(actor: &str, chain: Option<&str>, model: bool) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: model,
        allow_revoke: false,
    }
}

async fn call(core: &Core, context: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, context, &payload).await.unwrap()["result"].clone()
}

async fn pair(core: &Core) -> (OperationContext, OperationContext, String) {
    let alice_id = call(
        core,
        &context("alice", None, false),
        "chain.create",
        json!({}),
    )
    .await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let bob_id = call(
        core,
        &context("bob", None, false),
        "chain.create",
        json!({}),
    )
    .await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice = context("alice", Some(&alice_id), true);
    let bob = context("bob", Some(&bob_id), false);
    let source = call(
        core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"fixture"}]}}),
    )
    .await;
    let job = call(
        core,
        &alice,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":[]}),
    )
    .await;
    let cut = call(
        core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"initial"}),
    )
    .await;
    let checkpoint = call(
        core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":cut["cut_id"]}),
    )
    .await;
    let socket = call(
        core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],
        "acl":{"message":[bob_id],"signal":[bob_id],"spawn":[],"read":[]}}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (alice, bob, socket)
}

#[tokio::test]
async fn host_wait_releases_database_for_sender_and_obeys_timeout_and_authority() {
    let core = Core::new(Store::memory().await.unwrap());
    let (alice, bob, socket) = pair(&core).await;
    let waiting_core = core.clone();
    let waiting_context = alice.clone();
    let waiting = tokio::spawn(async move {
        waiting_core
            .operate(
                "signal.wait",
                &waiting_context,
                &json!({"signal_id":"wake-one","timeout_ms":2000}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!waiting.is_finished());
    let sent = tokio::time::timeout(
        Duration::from_secs(1),
        core.operate(
            "socket.signal",
            &bob,
            &json!({"socket_id":socket,"signal_id":"wake-one","action":"wake"}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(sent["result"]["receipt"]["signal_id"], "wake-one");
    let received = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received["result"]["receipt"]["signal_id"], "wake-one");
    assert_eq!(received["result"]["timed_out"], false);
    let immediate = call(
        &core,
        &alice,
        "signal.wait",
        json!({"signal_id":"wake-one","timeout_ms":0}),
    )
    .await;
    assert_eq!(immediate["receipt"]["signal_id"], "wake-one");
    let expired = call(
        &core,
        &alice,
        "signal.wait",
        json!({"signal_id":"missing","timeout_ms":100}),
    )
    .await;
    assert_eq!(expired["timed_out"], true);
    assert!(expired["waited_ms"].as_u64().unwrap() >= 80);
    assert!(
        core.operate(
            "signal.wait",
            &alice,
            &json!({"signal_id":"x","timeout_ms":30001})
        )
        .await
        .is_err()
    );
    let foreign = context("mallory", alice.chain_id.as_deref(), false);
    assert!(
        core.operate("signal.wait", &foreign, &json!({"signal_id":"wake-one"}))
            .await
            .is_err()
    );
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,"signal_id":"stop-one","action":"stop"}),
    )
    .await;
    let stopped = call(
        &core,
        &alice,
        "signal.wait",
        json!({"signal_id":"missing","timeout_ms":2000}),
    )
    .await;
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["timed_out"], false);
}

#[tokio::test]
async fn independent_core_instances_observe_durable_signals_without_shared_notifier() {
    let store = Store::memory().await.unwrap();
    let waiting_core = Core::new(store.clone());
    let sending_core = Core::new(store);
    let (alice, bob, socket) = pair(&waiting_core).await;
    let waiting_context = alice.clone();
    let waiting = tokio::spawn(async move {
        waiting_core
            .operate(
                "signal.wait",
                &waiting_context,
                &json!({"signal_id":"cross-instance","timeout_ms":3000}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    call(
        &sending_core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,"signal_id":"cross-instance","action":"wake"}),
    )
    .await;
    let result = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result["result"]["receipt"]["signal_id"], "cross-instance");
}

#[tokio::test]
async fn signal_cursor_waits_for_the_next_receipt_without_replaying_old_signals() {
    let core = Core::new(Store::memory().await.unwrap());
    let (alice, bob, socket) = pair(&core).await;
    let first = call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,
        "signal_id":"z-first","action":"wake"}),
    )
    .await;
    let second = call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,
        "signal_id":"a-second","action":"wake"}),
    )
    .await;
    let first_seq = first["receipt"]["sequence"].as_u64().unwrap();
    let second_seq = second["receipt"]["sequence"].as_u64().unwrap();
    assert!(second_seq > first_seq);
    let earliest = call(&core, &alice, "signal.wait", json!({"after_sequence":0})).await;
    assert_eq!(earliest["receipt"]["signal_id"], "z-first");
    assert_eq!(earliest["next_after_sequence"], first_seq);
    let next = call(
        &core,
        &alice,
        "signal.wait",
        json!({"after_sequence":first_seq}),
    )
    .await;
    assert_eq!(next["receipt"]["signal_id"], "a-second");
    assert_eq!(next["next_after_sequence"], second_seq);
    let empty = call(
        &core,
        &alice,
        "signal.wait",
        json!({"after_sequence":second_seq}),
    )
    .await;
    assert!(empty["receipt"].is_null());
    assert_eq!(empty["next_after_sequence"], second_seq);
    assert_eq!(empty["timed_out"], true);
    assert!(
        core.operate(
            "signal.wait",
            &alice,
            &json!({"signal_id":"z-first","after_sequence":0})
        )
        .await
        .is_err()
    );
    assert!(
        core.operate("signal.wait", &alice, &json!({"after_sequence":-1}))
            .await
            .is_err()
    );
    let foreign = context("mallory", alice.chain_id.as_deref(), false);
    assert!(
        core.operate("signal.wait", &foreign, &json!({"after_sequence":0}))
            .await
            .is_err()
    );

    let waiting_core = core.clone();
    let waiting_context = alice.clone();
    let waiting = tokio::spawn(async move {
        waiting_core
            .operate(
                "signal.wait",
                &waiting_context,
                &json!({"after_sequence":second_seq,"timeout_ms":2000}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,
        "signal_id":"a-second","action":"wake"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !waiting.is_finished(),
        "an idempotent replay is not a new Signal"
    );
    let third = call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,
        "signal_id":"middle-third","action":"wake"}),
    )
    .await;
    let third_seq = third["receipt"]["sequence"].as_u64().unwrap();
    let received = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received["result"]["receipt"]["signal_id"], "middle-third");
    assert_eq!(received["result"]["next_after_sequence"], third_seq);
}

#[tokio::test]
async fn host_waits_for_new_messages_without_consuming_or_skipping_them() {
    let core = Core::new(Store::memory().await.unwrap());
    let (alice, bob, socket) = pair(&core).await;
    let mut reader = alice.clone();
    reader.allow_write = false;
    let empty = call(&core, &reader, "message.wait", json!({"after_sequence":0})).await;
    assert!(empty["message"].is_null());
    assert_eq!(empty["next_after_sequence"], 0);
    assert_eq!(empty["timed_out"], true);
    assert!(
        core.operate("message.wait", &reader, &json!({}))
            .await
            .is_err()
    );
    assert!(
        core.operate(
            "message.wait",
            &reader,
            &json!({"after_sequence":0,"timeout_ms":30001})
        )
        .await
        .is_err()
    );
    let foreign = context("mallory", alice.chain_id.as_deref(), false);
    assert!(
        core.operate("message.wait", &foreign, &json!({"after_sequence":0}))
            .await
            .is_err()
    );

    let waiting_core = Core::new(core.store.clone());
    let waiting_context = reader.clone();
    let waiting = tokio::spawn(async move {
        waiting_core
            .operate(
                "message.wait",
                &waiting_context,
                &json!({"after_sequence":0,"timeout_ms":3000}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let first = call(
        &core,
        &bob,
        "socket.message",
        json!({"socket_id":socket,
        "request_id":"request-first","message_id":"message-first","text":"first","lifecycle":"V"}),
    )
    .await;
    let received = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let sequence = received["result"]["next_after_sequence"].as_u64().unwrap();
    assert_eq!(
        received["result"]["message"]["message"]["message_id"],
        "message-first"
    );
    assert!(received["result"]["message"]["consumed_by_cut"].is_null());
    assert_eq!(first["receipt"]["sequence"], sequence);
    let reread = call(&core, &reader, "message.wait", json!({"after_sequence":0})).await;
    assert_eq!(reread["message"]["message"]["message_id"], "message-first");

    let waiting_core = core.clone();
    let waiting_context = reader.clone();
    let waiting = tokio::spawn(async move {
        waiting_core
            .operate(
                "message.wait",
                &waiting_context,
                &json!({"after_sequence":sequence,"timeout_ms":2000}),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    call(
        &core,
        &bob,
        "socket.message",
        json!({"socket_id":socket,
        "request_id":"request-first","message_id":"message-first","text":"first","lifecycle":"V"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !waiting.is_finished(),
        "idempotent replay is not a new Message"
    );
    call(&core, &bob, "socket.message", json!({"socket_id":socket,
        "request_id":"request-second","message_id":"message-second","text":"second","lifecycle":"P"})).await;
    let received = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        received["result"]["message"]["message"]["message_id"],
        "message-second"
    );
    assert!(received["result"]["next_after_sequence"].as_u64().unwrap() > sequence);
}

#[tokio::test]
async fn concurrent_waits_are_bounded_and_cancelled_waits_release_capacity() {
    let core = Core::new(Store::memory().await.unwrap());
    let (alice, _bob, _socket) = pair(&core).await;
    let mut waits = Vec::new();
    for i in 0..32 {
        let waiting_core = core.clone();
        let context = alice.clone();
        waits.push(tokio::spawn(async move {
            waiting_core
                .operate(
                    "signal.wait",
                    &context,
                    &json!({"signal_id":format!("pending-{i}"),"timeout_ms":5000}),
                )
                .await
        }));
    }
    // Let the 32 spawned futures reserve their permits before the probe;
    // otherwise the probe itself can take a slot and make one waiter fail.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(waits.iter().all(|task| !task.is_finished()));
    let error = core
        .operate(
            "signal.wait",
            &alice,
            &json!({"signal_id":"capacity-probe","timeout_ms":1}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "signal_wait_capacity");
    let error = core
        .operate(
            "message.wait",
            &alice,
            &json!({"after_sequence":0,"timeout_ms":1}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "message_wait_capacity");
    for task in waits {
        task.abort();
        let _ = task.await;
    }
    let result = call(
        &core,
        &alice,
        "signal.wait",
        json!({"signal_id":"capacity-released","timeout_ms":1}),
    )
    .await;
    assert_eq!(result["timed_out"], true);
}

#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<Value>>>,
    started: Arc<Notify>,
    mixed: bool,
    wait_ms: u64,
    selector_by_cursor: bool,
}

async fn completion(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    let number = {
        let mut calls = mock.calls.lock().await;
        calls.push(body);
        calls.len()
    };
    let message = if number == 1 {
        mock.started.notify_one();
        let arguments = if mock.selector_by_cursor {
            json!({"after_sequence":0,"timeout_ms":mock.wait_ms})
        } else {
            json!({"signal_id":"worker-wake","timeout_ms":mock.wait_ms})
        };
        let mut calls = vec![
            json!({"id":"wait-tool","type":"function","function":{"name":"sic_signal_wait",
            "arguments":arguments.to_string()}}),
        ];
        if mock.mixed {
            calls.push(json!({"id":"decision-tool","type":"function","function":{"name":"sic_decision_read",
            "arguments":"{\"view\":\"Working on\"}"}}));
        }
        json!({"role":"assistant","content":null,"tool_calls":calls})
    } else {
        json!({"role":"assistant","content":"continued after signal"})
    };
    let finish = if number == 1 { "tool_calls" } else { "stop" };
    Json(
        json!({"model":"fixed-model","usage":{"prompt_tokens":9,"completion_tokens":4},
        "choices":[{"finish_reason":finish,"message":message}]}),
    )
}

async fn model_core(mixed: bool) -> (Core, Mock, tokio::task::JoinHandle<()>) {
    model_core_with_store(mixed, Store::memory().await.unwrap()).await
}

async fn model_core_with_store(
    mixed: bool,
    store: Store,
) -> (Core, Mock, tokio::task::JoinHandle<()>) {
    model_core_with_selector(mixed, store, false, 2000).await
}

async fn model_core_with_selector(
    mixed: bool,
    store: Store,
    selector_by_cursor: bool,
    wait_ms: u64,
) -> (Core, Mock, tokio::task::JoinHandle<()>) {
    let mock = Mock {
        calls: Arc::new(Mutex::new(Vec::new())),
        started: Arc::new(Notify::new()),
        mixed,
        wait_ms,
        selector_by_cursor,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_mock = mock.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(server_mock),
        )
        .await
        .unwrap();
    });
    let profile = json!({"model":"openai/fixed-model","api_base":format!("http://{address}/v1"),"api_key_env":"FIXTURE_KEY",
        "tokenizer_model":"host-o200k","context_window":1000000,"max_input_tokens":1000000,
        "max_output_tokens":4096,"default_output_tokens":1024,"timeout_seconds":5});
    let core = Core::with_model_config_and_resolver(
        store,
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    (core, mock, server)
}

#[tokio::test]
async fn worker_waits_without_locking_sender_and_continues_with_exact_receipt() {
    let (core, mock, server) = model_core(false).await;
    let (alice, bob, socket) = pair(&core).await;
    let worker_core = core.clone();
    let worker_context = alice.clone();
    let worker = tokio::spawn(async move {
        worker_core
            .operate("worker.run", &worker_context, &json!({}))
            .await
    });
    mock.started.notified().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!worker.is_finished());
    let during_wait = call(&core, &alice, "accounting.get", json!({})).await;
    assert_eq!(during_wait["known_prompt_tokens"], 9);
    assert_eq!(during_wait["known_completion_tokens"], 4);
    assert_eq!(during_wait["pending_worker_actions"], 1);
    assert_eq!(during_wait["unresolved_attempts"], 0);
    tokio::time::timeout(
        Duration::from_secs(1),
        core.operate(
            "socket.signal",
            &bob,
            &json!({"socket_id":socket,"signal_id":"worker-wake","action":"wake"}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        completed["result"]["tool_results"][0]["result"]["result"]["receipt"]["signal_id"],
        "worker-wake"
    );
    let completed_ledger = call(&core, &alice, "accounting.get", json!({})).await;
    assert_eq!(completed_ledger["known_prompt_tokens"], 9);
    assert_eq!(completed_ledger["known_completion_tokens"], 4);
    assert_eq!(completed_ledger["pending_worker_actions"], 0);
    assert_eq!(
        call(&core, &alice, "worker.run", json!({})).await["content"],
        "continued after signal"
    );
    server.abort();
}

#[tokio::test]
async fn worker_can_wait_for_the_next_signal_without_knowing_its_id() {
    let (core, mock, server) =
        model_core_with_selector(false, Store::memory().await.unwrap(), true, 2000).await;
    let (alice, bob, socket) = pair(&core).await;
    let worker_core = core.clone();
    let worker_context = alice.clone();
    let worker = tokio::spawn(async move {
        worker_core
            .operate("worker.run", &worker_context, &json!({}))
            .await
    });
    mock.started.notified().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if call(&core, &alice, "accounting.get", json!({})).await["pending_worker_actions"] == 1 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let sent = call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,
        "signal_id":"not-known-in-advance","action":"wake"}),
    )
    .await;
    let sequence = sent["receipt"]["sequence"].as_u64().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let received = &result["result"]["tool_results"][0]["result"]["result"];
    assert_eq!(received["receipt"]["signal_id"], "not-known-in-advance");
    assert_eq!(received["next_after_sequence"], sequence);
    assert_eq!(
        call(&core, &alice, "worker.run", json!({})).await["content"],
        "continued after signal"
    );
    let requests = mock.calls.lock().await;
    assert!(
        requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "tool"
                    && message["content"].as_str().is_some_and(|content| {
                        content.contains("not-known-in-advance")
                            && content.contains("next_after_sequence")
                    })
            })
    );
    server.abort();
}

#[tokio::test]
async fn worker_can_poll_next_signal_with_zero_timeout() {
    let (core, _mock, server) =
        model_core_with_selector(false, Store::memory().await.unwrap(), true, 0).await;
    let (alice, _, _) = pair(&core).await;
    let result = call(&core, &alice, "worker.run", json!({})).await;
    let received = &result["tool_results"][0]["result"]["result"];
    assert_eq!(received["timed_out"], true);
    assert_eq!(received["next_after_sequence"], 0);
    server.abort();
}

#[tokio::test]
async fn interrupted_wait_retains_provider_usage_for_a_reopened_core() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("signal-wait.db");
    let (core, mock, server) =
        model_core_with_store(false, Store::create(&db_path).await.unwrap()).await;
    let (alice, _bob, _socket) = pair(&core).await;
    let worker_core = core.clone();
    let worker_context = alice.clone();
    let worker = tokio::spawn(async move {
        worker_core
            .operate("worker.run", &worker_context, &json!({}))
            .await
    });
    mock.started.notified().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let ledger = call(&core, &alice, "accounting.get", json!({})).await;
        if ledger["pending_worker_actions"] == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "provider observation was not persisted"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    worker.abort();
    let _ = worker.await;
    drop(core);
    let reopened = Core::new(Store::open(&db_path).await.unwrap());
    let ledger = call(&reopened, &alice, "accounting.get", json!({})).await;
    assert_eq!(ledger["known_prompt_tokens"], 9);
    assert_eq!(ledger["known_completion_tokens"], 4);
    assert_eq!(ledger["pending_worker_actions"], 1);
    assert_eq!(ledger["superseded_worker_actions"], 0);
    assert_eq!(ledger["unresolved_attempts"], 0);
    assert_eq!(ledger["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &reopened,
            &alice,
            "runtime.abandon",
            json!({"kind":"worker"})
        )
        .await["abandoned"],
        true
    );
    let abandoned = call(&reopened, &alice, "accounting.get", json!({})).await;
    assert_eq!(abandoned["known_prompt_tokens"], 9);
    assert_eq!(abandoned["known_completion_tokens"], 4);
    assert_eq!(abandoned["pending_worker_actions"], 0);
    assert_eq!(abandoned["superseded_worker_actions"], 1);
    assert_eq!(abandoned["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(
        abandoned["attempts"][0]["observation"],
        "provider_completed_claim_superseded"
    );
    server.abort();
}

#[tokio::test]
async fn killed_service_preserves_wait_usage_and_requires_explicit_abandon() {
    async fn ready(client: &reqwest::Client, base: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if client
                .get(format!("{base}/health"))
                .bearer_auth("fixture-bearer")
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "service did not start"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    async fn operation(
        client: &reqwest::Client,
        base: &str,
        chain: &str,
        name: &str,
        payload: Value,
    ) -> Value {
        let response = client
            .post(format!("{base}/api/v2/operations"))
            .bearer_auth("fixture-bearer")
            .json(
                &json!({"operation":name,"context":{"actor":"alice","chain_id":chain,
                "allow_write":true,"allow_model":true},"payload":payload}),
            )
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "operation {name} failed");
        response.json::<Value>().await.unwrap()["result"].clone()
    }

    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("crash.db");
    let store = Store::create(&db).await.unwrap();
    let core = Core::new(store.clone());
    let (alice, _, _) = pair(&core).await;
    let chain = alice.chain_id.unwrap();
    store.close().await;
    drop(core);
    drop(store);

    let mock = Mock {
        calls: Arc::new(Mutex::new(Vec::new())),
        started: Arc::new(Notify::new()),
        mixed: false,
        wait_ms: 15000,
        selector_by_cursor: false,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_address = listener.local_addr().unwrap();
    let server_mock = mock.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(server_mock),
        )
        .await
        .unwrap();
    });
    let profile = json!({"model":"openai/fixed-model","api_base":format!("http://{mock_address}/v1"),
        "api_key_env":"FIXTURE_KEY","tokenizer_model":"host-o200k","context_window":1000000,
        "max_input_tokens":1000000,"max_output_tokens":4096,"default_output_tokens":1024,"timeout_seconds":5});
    let config = directory.path().join("model.json");
    std::fs::write(
        &config,
        json!({"roles":{"worker":profile,"editor":profile}}).to_string(),
    )
    .unwrap();
    let port_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = port_probe.local_addr().unwrap();
    drop(port_probe);
    let base = format!("http://{address}");
    let launch = || {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_sic-core-rust"));
        command
            .arg("serve")
            .arg(&db)
            .arg(address.to_string())
            .arg("alice")
            .arg("FIXTURE_BEARER")
            .arg("--allow-model")
            .arg("--model-config")
            .arg(&config)
            .env("FIXTURE_BEARER", "fixture-bearer")
            .env("FIXTURE_KEY", "fixture-key")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut service = launch();
    ready(&client, &base).await;
    let worker =
        {
            let client = client.clone();
            let base = base.clone();
            let chain = chain.clone();
            tokio::spawn(async move {
                client
                .post(format!("{base}/api/v2/operations"))
                .bearer_auth("fixture-bearer")
                .json(&json!({"operation":"worker.run","context":{"actor":"alice","chain_id":chain,
                    "allow_write":true,"allow_model":true},"payload":{}}))
                .send()
                .await
            })
        };
    tokio::time::timeout(Duration::from_secs(5), mock.started.notified())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ledger = operation(&client, &base, &chain, "accounting.get", json!({})).await;
        if ledger["pending_worker_actions"] == 1 {
            assert_eq!(ledger["known_prompt_tokens"], 9);
            assert_eq!(ledger["known_completion_tokens"], 4);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "observation not persisted"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    service.kill().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let mut restarted = launch();
    ready(&client, &base).await;
    let ledger = operation(&client, &base, &chain, "accounting.get", json!({})).await;
    assert_eq!(ledger["known_prompt_tokens"], 9);
    assert_eq!(ledger["known_completion_tokens"], 4);
    assert_eq!(ledger["pending_worker_actions"], 1);
    assert_eq!(mock.calls.lock().await.len(), 1);
    let abandoned = operation(
        &client,
        &base,
        &chain,
        "runtime.abandon",
        json!({"kind":"worker"}),
    )
    .await;
    assert_eq!(abandoned["abandoned"], true);
    let ledger = operation(&client, &base, &chain, "accounting.get", json!({})).await;
    assert_eq!(ledger["pending_worker_actions"], 0);
    assert_eq!(ledger["superseded_worker_actions"], 1);
    assert_eq!(ledger["known_prompt_tokens"], 9);
    let resumed = operation(&client, &base, &chain, "worker.run", json!({})).await;
    assert_eq!(resumed["content"], "continued after signal");
    let ledger = operation(&client, &base, &chain, "accounting.get", json!({})).await;
    assert_eq!(ledger["known_prompt_tokens"], 18);
    assert_eq!(ledger["known_completion_tokens"], 8);
    assert_eq!(mock.calls.lock().await.len(), 2);
    restarted.kill().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn abandoning_worker_interrupts_pending_signal_wait_and_keeps_usage() {
    let (core, mock, server) = model_core(false).await;
    let (alice, _bob, _socket) = pair(&core).await;
    let worker_core = core.clone();
    let worker_context = alice.clone();
    let worker = tokio::spawn(async move {
        worker_core
            .operate("worker.run", &worker_context, &json!({}))
            .await
    });
    mock.started.notified().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        call(&core, &alice, "runtime.abandon", json!({"kind":"worker"})).await["abandoned"],
        true
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let ledger = call(&core, &alice, "accounting.get", json!({})).await;
    assert_eq!(ledger["known_prompt_tokens"], 9);
    assert_eq!(ledger["known_completion_tokens"], 4);
    assert_eq!(ledger["unresolved_attempts"], 0);
    server.abort();
}

#[tokio::test]
async fn mixed_nonzero_wait_returns_tool_error_without_delaying_other_actions() {
    let (core, _mock, server) =
        model_core_with_selector(true, Store::memory().await.unwrap(), false, 10_000).await;
    let (alice, _bob, _socket) = pair(&core).await;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        core.operate("worker.run", &alice, &json!({})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        result["result"]["tool_results"][0]["result"]["code"],
        "signal_wait_requires_single_call"
    );
    assert_eq!(result["result"]["tool_results"][1]["result"]["ok"], true);
    server.abort();
}
