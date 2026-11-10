use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    peer_tasks,
    storage::Store,
};

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: false,
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

async fn publish(core: &Core, ctx: &OperationContext, text: &str, id: &str) -> Value {
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
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":id}),
    )
    .await
}

async fn fixture(core: &Core) -> (String, String, String, String) {
    let alice_route = make_chain(core, "alice").await;
    let bob_route = make_chain(core, "bob").await;
    let alice = context("alice", Some(&alice_route));
    let published = publish(core, &alice, "checkpoint source", "checkpoint-cut").await;
    let checkpoint = call(
        core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":published["cut_id"]}),
    )
    .await;
    let socket = call(
        core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[],"signal":[],"spawn":["bob"],"read":[]
        }}),
    )
    .await;
    (
        alice_route,
        bob_route,
        socket["socket_id"].as_str().unwrap().to_owned(),
        checkpoint["checkpoint_id"].as_str().unwrap().to_owned(),
    )
}

async fn spawn(core: &Core, parent: &str, socket: &str, request: &str) -> Value {
    call(
        core,
        &context("bob", Some(parent)),
        "socket.spawn",
        json!({"socket_id":socket,"request_id":request,"message_id":format!("message-{request}"),
              "text":"review the referenced snapshot","lifecycle":"P"}),
    )
    .await
}

#[tokio::test]
async fn socket_spawn_records_a_stable_pending_task_and_replay_is_idempotent() {
    let core = Core::new(Store::memory().await.unwrap());
    let (_alice_route, parent, socket, checkpoint) = fixture(&core).await;
    let first = spawn(&core, &parent, &socket, "spawn-stable-1").await;
    assert_eq!(first["task_queued"], true);
    assert_eq!(first["relationship"], "peer");
    assert_eq!(first["task_id"].as_str().unwrap().is_empty(), false);
    let task_id = first["task_id"].as_str().unwrap();
    let repeated = core
        .operate(
            "socket.spawn",
            &context("bob", Some(&parent)),
            &json!({"socket_id":socket,"request_id":"spawn-stable-1","message_id":"message-spawn-stable-1",
                   "text":"review the referenced snapshot","lifecycle":"P"}),
        )
        .await
        .unwrap();
    assert_eq!(repeated["evidence"]["replayed"], true);
    assert_eq!(repeated["result"]["task_id"], task_id);

    let parent_tasks = call(
        &core,
        &context("bob", Some(&parent)),
        "peer.tasks",
        json!({}),
    )
    .await;
    assert_eq!(parent_tasks["tasks"].as_array().unwrap().len(), 1);
    let task = &parent_tasks["tasks"][0];
    assert_eq!(task["task_id"], task_id);
    assert_eq!(task["state"], "pending");
    assert_eq!(task["requester_chain_id"], parent);
    assert_eq!(task["peer_chain_id"], first["chain_id"]);
    assert_eq!(task["socket_id"], socket);
    assert_eq!(task["checkpoint_id"], checkpoint);
    assert_eq!(task["request_id"], "spawn-stable-1");
    assert_eq!(task["message_id"], "message-spawn-stable-1");
    assert_eq!(task["lifecycle"], "P");

    let child_tasks = call(
        &core,
        &context("bob", Some(first["chain_id"].as_str().unwrap())),
        "peer.tasks",
        json!({}),
    )
    .await;
    assert_eq!(child_tasks["tasks"], parent_tasks["tasks"]);
}

#[tokio::test]
async fn peer_claim_is_atomic_scoped_and_replay_never_regrants_execution() {
    let core = Core::new(Store::memory().await.unwrap());
    let (_alice, parent, socket, _) = fixture(&core).await;
    let spawned = spawn(&core, &parent, &socket, "spawn-claim-race").await;
    let task_id = spawned["task_id"].as_str().unwrap().to_owned();

    let charlie_route = make_chain(&core, "charlie").await;
    let unauthorized = core
        .operate(
            "peer.task.claim",
            &context("charlie", Some(&charlie_route)),
            &json!({"task_id":task_id,"claim_id":"intruder"}),
        )
        .await;
    assert!(unauthorized.is_err());
    let charlie_tasks = call(
        &core,
        &context("charlie", Some(&charlie_route)),
        "peer.tasks",
        json!({}),
    )
    .await;
    assert!(charlie_tasks["tasks"].as_array().unwrap().is_empty());

    let ctx = context("bob", Some(&parent));
    let mut read_only_host = context("bob", Some(&parent));
    read_only_host.allow_write = false;
    let read_only_tasks = call(&core, &read_only_host, "peer.tasks", json!({})).await;
    assert_eq!(read_only_tasks["tasks"].as_array().unwrap().len(), 1);
    let payload_a = json!({"task_id":task_id,"claim_id":"claim-a"});
    let payload_b = json!({"task_id":task_id,"claim_id":"claim-b"});
    let claim_a = core.operate("peer.task.claim", &ctx, &payload_a);
    let claim_b = core.operate("peer.task.claim", &ctx, &payload_b);
    let (a, b) = tokio::join!(claim_a, claim_b);
    let winners: Vec<Value> = [a, b]
        .into_iter()
        .filter_map(Result::ok)
        .map(|result| result["result"].clone())
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(winners[0]["execution_permit"], true);
    assert_eq!(winners[0]["state"], "claimed");
    let winning_claim = winners[0]["claim_id"].as_str().unwrap();

    let replay = call(
        &core,
        &ctx,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":winning_claim}),
    )
    .await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["execution_permit"], false);
    assert!(
        core.operate(
            "peer.task.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"claim-new"}),
        )
        .await
        .is_err()
    );
    let started = call(
        &core,
        &ctx,
        "peer.task.advance",
        json!({"task_id":task_id,"claim_id":winning_claim,"event_id":"start-event",
              "expected_revision":1,"state":"running"}),
    )
    .await;
    assert_eq!(started["state"], "running");
    assert_eq!(started["revision"], 2);
    let repeated = call(
        &core,
        &ctx,
        "peer.task.advance",
        json!({"task_id":task_id,"claim_id":winning_claim,"event_id":"start-event",
              "expected_revision":1,"state":"running"}),
    )
    .await;
    assert_eq!(repeated["replayed"], true);
    assert_eq!(repeated["execution_permit"], false);
    let completed = call(
        &core,
        &ctx,
        "peer.task.advance",
        json!({"task_id":task_id,"claim_id":winning_claim,"event_id":"complete-event",
              "expected_revision":2,"state":"completed"}),
    )
    .await;
    assert_eq!(completed["state"], "host_reported_completed");
    assert_eq!(
        completed["completion_attestation"],
        json!({"kind":"host_only","evidence_verified":false})
    );
    assert_eq!(completed["revision"], 3);
}

#[tokio::test]
async fn claimed_peer_task_survives_reopen_and_can_only_be_isolated_as_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("peer-tasks.sqlite3");
    let store = Store::create(&path).await.unwrap();
    let core = Core::new(store.clone());
    let (_alice, parent, socket, _) = fixture(&core).await;
    let spawned = spawn(&core, &parent, &socket, "spawn-restart-1").await;
    let task_id = spawned["task_id"].as_str().unwrap().to_owned();
    let ctx = context("bob", Some(&parent));
    let claim = call(
        &core,
        &ctx,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":"stable-host-claim"}),
    )
    .await;
    assert_eq!(claim["execution_permit"], true);
    store.close().await;
    drop(core);
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    let restarted = Core::new(reopened.clone());
    let restored = call(&restarted, &ctx, "peer.tasks", json!({})).await;
    assert_eq!(restored["tasks"][0]["state"], "claimed");
    let replay = call(
        &restarted,
        &ctx,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":"stable-host-claim"}),
    )
    .await;
    assert_eq!(replay["execution_permit"], false);
    assert!(
        restarted
            .operate(
                "peer.task.claim",
                &ctx,
                &json!({"task_id":task_id,"claim_id":"after-restart"}),
            )
            .await
            .is_err()
    );

    assert!(
        restarted
            .operate(
                "peer.task.unknown",
                &ctx,
                &json!({"task_id":task_id,"claim_id":"stable-host-claim","event_id":"bad-reason",
                   "expected_revision":1,"reason":17}),
            )
            .await
            .is_err()
    );
    let unknown = call(
        &restarted,
        &ctx,
        "peer.task.unknown",
        json!({"task_id":task_id,"claim_id":"stable-host-claim","event_id":"reconcile-1",
              "expected_revision":1,"reason":"host outcome uncertain after restart"}),
    )
    .await;
    assert_eq!(unknown["state"], "unknown");
    assert_eq!(unknown["revision"], 2);
    assert!(
        restarted
            .operate(
                "peer.task.advance",
                &ctx,
                &json!({"task_id":task_id,"claim_id":"stable-host-claim","event_id":"complete-1",
                   "expected_revision":2,"state":"completed"}),
            )
            .await
            .is_err()
    );
    reopened.close().await;
}

#[tokio::test]
async fn caller_owned_spawn_record_transaction_rolls_back_completely() {
    let store = Store::memory().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    peer_tasks::record_spawn_in_transaction(
        &mut tx,
        "task-rollback",
        "alice",
        "parent-route",
        "child-route",
        "socket-id",
        "checkpoint-id",
        "spawn-request",
        "message-id",
        "P",
    )
    .await
    .unwrap();
    drop(tx); // Caller failure rolls back task state with the enclosing spawn.

    let mut tx = store.begin().await.unwrap();
    let records =
        peer_tasks::list_for_chain_in_transaction(&mut tx, "alice", "parent-route", 0, 100)
            .await
            .unwrap();
    assert!(records.is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn failed_spawn_record_rolls_back_child_route_and_request_receipt_together() {
    let core = Core::new(Store::memory().await.unwrap());
    let (_alice, parent, socket, _) = fixture(&core).await;
    let mut tx = core.store.begin().await.unwrap();
    peer_tasks::initialize_in_transaction(&mut tx)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_peer_task BEFORE INSERT ON sic_peer_tasks BEGIN SELECT RAISE(ABORT,'injected peer task failure'); END")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let payload = json!({"socket_id":socket,"request_id":"atomic-spawn-1","message_id":"atomic-message-1",
                         "text":"atomic peer task","lifecycle":"V"});
    assert!(
        core.operate("socket.spawn", &context("bob", Some(&parent)), &payload)
            .await
            .is_err()
    );
    let routes = call(&core, &context("bob", None), "chain.list", json!({})).await;
    assert_eq!(routes["routes"].as_array().unwrap().len(), 1);
    let tasks = call(
        &core,
        &context("bob", Some(&parent)),
        "peer.tasks",
        json!({}),
    )
    .await;
    assert!(tasks["tasks"].as_array().unwrap().is_empty());

    let mut tx = core.store.begin().await.unwrap();
    sqlx::query("DROP TRIGGER reject_peer_task")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let succeeded = core
        .operate("socket.spawn", &context("bob", Some(&parent)), &payload)
        .await
        .unwrap();
    assert_eq!(succeeded["evidence"]["replayed"], false);
    assert_eq!(succeeded["result"]["task_queued"], true);
    assert_eq!(
        call(
            &core,
            &context("bob", Some(&parent)),
            "peer.tasks",
            json!({}),
        )
        .await["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
