use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn ctx(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}

async fn op(core: &Core, c: &OperationContext, name: &str, p: Value) -> Value {
    core.operate(name, c, &p)
        .await
        .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))["result"]
        .clone()
}

async fn chain(core: &Core, actor: &str) -> String {
    op(core, &ctx(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish(core: &Core, c: &OperationContext, text: &str, key: &str) {
    let src = op(
        core,
        c,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let job = op(
        core,
        c,
        "edit.program",
        json!({"source_id":src["source_id"],"rules":[]}),
    )
    .await;
    let state = op(core, c, "chain.get", json!({})).await;
    op(
        core,
        c,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await;
}

#[tokio::test]
async fn each_publication_atomically_refreshes_latest_board_i_without_copying_it_into_cb() {
    let core = Core::new(Store::memory().await.unwrap());
    let alice_id = chain(&core, "alice").await;
    let bob_id = chain(&core, "bob").await;
    let alice = ctx("alice", Some(&alice_id));
    let bob = ctx("bob", Some(&bob_id));
    let created = op(
        &core,
        &alice,
        "board.create",
        json!({"acl":{
            "readers":[alice_id,bob_id], "writers":[bob_id], "claimers":[], "managers":[alice_id]
        }}),
    )
    .await;
    let board_id = created["board_id"].as_str().unwrap();
    op(&core, &bob, "board.entry.text", json!({"board_id":board_id,"expected_revision":1,"idempotency_key":"b1","text":"board-note-one"})).await;

    publish(&core, &alice, "source-body-one", "p1").await;
    let first = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(first["view"]["instant"]["board"]["revision"], 2);
    assert!(
        first["view"]["cbs"][0]["body"]
            .as_str()
            .unwrap()
            .contains("source-body-one")
    );
    assert!(
        !first["view"]["cbs"][0]
            .to_string()
            .contains("board-note-one")
    );

    op(&core, &bob, "board.entry.text", json!({"board_id":board_id,"expected_revision":2,"idempotency_key":"b2","text":"board-note-two"})).await;
    // Board changes do not rewrite an already-published cut. The next CB publication
    // is the synchronization boundary and must capture the then-current I.
    let unchanged = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(unchanged["view"]["instant"]["board"]["revision"], 2);
    publish(&core, &alice, "source-body-two", "p2").await;
    let second = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(second["view"]["instant"]["board"]["revision"], 3);
    assert!(
        second["view"]["instant"]["board"]
            .to_string()
            .contains("board-note-two")
    );
    let cbs = second["view"]["cbs"].as_array().unwrap();
    assert_eq!(cbs.len(), 2);
    assert!(cbs.iter().all(|cb| !cb.to_string().contains("board-note-")));
    assert!(cbs.iter().all(|cb| !cb.to_string().contains("\"board\"")));

    op(&core, &bob, "board.bind", json!({"board_id":board_id})).await;

    // Revoke Bob while he still has a copied binding; authorization is rechecked
    // against his own chain on every request, and unbind remains available.
    op(
        &core,
        &alice,
        "board.acl.update",
        json!({"expected_revision":3,"idempotency_key":"revoke-bob","acl":{
            "readers":[alice_id], "writers":[], "claimers":[], "managers":[alice_id]
        }}),
    )
    .await;
    assert_eq!(
        core.operate("board.get", &bob, &json!({"board_id":board_id}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        core.operate("cut.get", &bob, &json!({}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    op(&core, &bob, "board.unbind", json!({})).await;
    assert_eq!(
        core.operate("board.bind", &bob, &json!({"board_id":board_id}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    let bob_view = op(&core, &bob, "chain.get", json!({})).await;
    assert!(bob_view["board_id"].is_null());
    assert!(!bob_view["contexts"].to_string().contains("\"board\":"));
}

#[tokio::test]
async fn fork_inherits_board_snapshot_but_not_the_parent_chains_board_acl() {
    let core = Core::new(Store::memory().await.unwrap());
    let alice_id = chain(&core, "alice").await;
    let bob_id = chain(&core, "bob").await;
    let alice = ctx("alice", Some(&alice_id));
    let bob = ctx("bob", Some(&bob_id));
    let board = op(
        &core,
        &alice,
        "board.create",
        json!({"acl":{
            "readers":[alice_id], "writers":[], "claimers":[], "managers":[alice_id]
        }}),
    )
    .await;
    let board_id = board["board_id"].as_str().unwrap();
    publish(&core, &alice, "parent-only-body", "parent-pub").await;
    let cut = op(&core, &alice, "cut.get", json!({})).await;
    let checkpoint = op(
        &core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":cut["view"]["cut_id"]}),
    )
    .await;
    let socket = op(
        &core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[], "signal":[], "spawn":[bob_id], "read":[]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let spawned = op(&core, &bob, "socket.spawn", json!({"socket_id":socket,"request_id":"fork-1","message_id":"task-1","text":"fork task","lifecycle":"P"})).await;
    let child_id = spawned["chain_id"].as_str().unwrap();
    let child = ctx("bob", Some(child_id));

    assert_eq!(
        core.operate("cut.get", &child, &json!({}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        core.operate("board.get", &child, &json!({}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );

    // The child can drop the inherited board binding even though its new chain
    // has no Board grant. This removes the stale I view from all child contexts.
    op(&core, &child, "board.unbind", json!({})).await;
    let detached = op(&core, &child, "chain.get", json!({})).await;
    assert!(detached["board_id"].is_null());
    assert!(!detached.to_string().contains(board_id));
    assert!(core.operate("cut.get", &child, &json!({})).await.is_ok());
}

#[derive(Clone)]
struct BoardMock {
    requests: Arc<Mutex<Vec<Value>>>,
    task_id: String,
}

async fn board_model(State(mock): State<BoardMock>, Json(body): Json<Value>) -> Json<Value> {
    let mut requests = mock.requests.lock().await;
    requests.push(body);
    let task_id = mock.task_id.clone();
    let (finish, message) = match requests.len() {
        1 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"claim-call","type":"function","function":{
            "name":"sic_board_claim","arguments":json!({"task_id":task_id,"expected_revision":2}).to_string()}}]}),
        ),
        2 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"fake-complete","type":"function","function":{
            "name":"sic_board_progress","arguments":json!({"task_id":task_id,"expected_revision":3,"status":"done"}).to_string()}}]}),
        ),
        _ => (
            "stop",
            json!({"role":"assistant","content":"The task remains claimed."}),
        ),
    };
    Json(
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":finish,"message":message}]}),
    )
}

#[tokio::test]
async fn model_can_claim_and_report_progress_but_cannot_forge_task_completion() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let profile = json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":format!("http://{addr}/v1"),
        "api_key_env":"LOCAL_FIXTURE_KEY","tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native",
        "context_window":100000,"max_input_tokens":100000,"max_output_tokens":4096,"default_output_tokens":512,"timeout_seconds":5});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-only".into())),
    )
    .unwrap();
    let mut alice = ctx("alice", None);
    alice.allow_model = true;
    let chain_id = op(&core, &alice, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    alice.chain_id = Some(chain_id.clone());
    op(&core, &alice, "board.create", json!({"acl":{
        "readers":[chain_id.clone()], "writers":[chain_id.clone()], "claimers":[chain_id.clone()], "managers":[chain_id]
    }})).await;
    let task = op(&core, &alice, "board.task.create", json!({"expected_revision":1,"idempotency_key":"fixture-task","title":"Do the work","detail":{}})).await;
    let mock = BoardMock {
        requests: Arc::new(Mutex::new(Vec::new())),
        task_id: task["task"]["task_id"].as_str().unwrap().to_owned(),
    };
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(board_model))
                .with_state(mock.clone()),
        )
        .into_future(),
    );

    let claimed = core
        .operate("worker.run", &alice, &json!({}))
        .await
        .unwrap();
    let forged = core
        .operate("worker.run", &alice, &json!({}))
        .await
        .unwrap();
    assert!(
        forged["result"]["tool_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["result"]["ok"] == false)
    );
    let state = op(&core, &alice, "board.get", json!({})).await;
    assert_eq!(state["tasks"][0]["status"], "claimed");
    assert_eq!(state["revision"], 3);
    let claim_results = claimed["result"]["tool_results"].as_array().unwrap();
    assert!(
        claim_results
            .iter()
            .any(|row| row["result"]["result"]["task"]["status"] == "claimed")
    );
    let final_answer = core
        .operate("worker.run", &alice, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        final_answer["result"]["content"],
        "The task remains claimed."
    );
    let requests = mock.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request["model"] == "deepseek-v4.1-flash-eco")
    );
    drop(requests);
    server.abort();
}
