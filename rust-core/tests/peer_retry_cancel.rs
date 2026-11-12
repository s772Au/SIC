use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
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
    core.operate(operation, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{operation} failed: {error:?}"))["result"]
        .clone()
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
async fn fixture(core: &Core) -> (String, String, String) {
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
async fn cancelled_peer_task_is_terminal_and_a_retry_needs_a_new_request() {
    let core = Core::new(Store::memory().await.unwrap());
    let (parent, socket, _) = fixture(&core).await;
    let spawned = spawn(&core, &parent, &socket, "spawn-cancel-1").await;
    let task_id = spawned["task_id"].as_str().unwrap().to_owned();
    let ctx = context("bob", Some(&parent));
    let claim = call(
        &core,
        &ctx,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":"claim-one"}),
    )
    .await;
    assert_eq!(claim["execution_permit"], true);
    call(
        &core,
        &ctx,
        "peer.task.advance",
        json!({"task_id":task_id,"claim_id":"claim-one","event_id":"start-one",
               "expected_revision":1,"state":"running"}),
    )
    .await;
    let cancelled = call(
        &core,
        &ctx,
        "peer.task.unknown",
        json!({"task_id":task_id,"claim_id":"claim-one","event_id":"cancel-one",
               "expected_revision":2,"reason":"host cancelled"}),
    )
    .await;
    assert_eq!(cancelled["state"], "unknown");
    assert_eq!(cancelled["revision"], 3);

    // A late result cannot overwrite the terminal state.
    let late = core
        .operate(
            "peer.task.reply",
            &ctx,
            &json!({"task_id":task_id,"reply_id":"late-reply",
                    "expected_attempt_id":"late-attempt","socket_id":socket}),
        )
        .await
        .unwrap_err();
    assert!(late.message.contains("prepare receipt"), "{late:?}");
    let late_advance = core
        .operate(
            "peer.task.advance",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"claim-one","event_id":"late-event",
                    "expected_revision":3,"state":"completed"}),
        )
        .await;
    assert!(late_advance.is_err());
    // The original permit can never be re-granted.
    let reclaim = core
        .operate(
            "peer.task.claim",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"claim-two"}),
        )
        .await
        .unwrap_err();
    assert!(
        reclaim.message.contains("already claimed or terminal"),
        "{reclaim:?}"
    );

    // Explicit retry: a new request creates a distinguishable new attempt.
    let retry = spawn(&core, &parent, &socket, "spawn-cancel-2").await;
    let retry_id = retry["task_id"].as_str().unwrap().to_owned();
    assert_ne!(retry_id, task_id);
    assert_eq!(retry["task_queued"], true);
    let tasks = call(&core, &ctx, "peer.tasks", json!({})).await;
    let rows = tasks["tasks"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|row| row["task_id"] == task_id).unwrap()["state"],
        "unknown"
    );
    assert_eq!(
        rows.iter().find(|row| row["task_id"] == retry_id).unwrap()["state"],
        "pending"
    );
    // Replaying the original request never creates a third task.
    let replay = spawn(&core, &parent, &socket, "spawn-cancel-1").await;
    assert_eq!(replay["task_id"], task_id);
    let tasks = call(&core, &ctx, "peer.tasks", json!({})).await;
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 2);
    let retried = call(
        &core,
        &ctx,
        "peer.task.claim",
        json!({"task_id":retry_id,"claim_id":"claim-retry"}),
    )
    .await;
    assert_eq!(retried["execution_permit"], true);
    assert_eq!(retried["state"], "claimed");
}

#[tokio::test]
async fn prepared_task_can_be_isolated_unknown_before_a_late_reply() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let (parent, socket, _) = fixture(&core).await;
    let spawned = spawn(&core, &parent, &socket, "spawn-prepared-cancel").await;
    let task_id = spawned["task_id"].as_str().unwrap().to_owned();
    let ctx = context("bob", Some(&parent));
    call(
        &core,
        &ctx,
        "peer.task.claim",
        json!({"task_id":task_id,"claim_id":"prepared-claim"}),
    )
    .await;
    let mut tx = store.begin().await.unwrap();
    sqlx::query("UPDATE sic_peer_tasks SET state='prepared',revision=2 WHERE task_id=?")
        .bind(&task_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let unknown = call(
        &core,
        &ctx,
        "peer.task.unknown",
        json!({"task_id":task_id,"claim_id":"prepared-claim","event_id":"prepared-cancel",
               "expected_revision":2,"reason":"host cancelled after child dispatch"}),
    )
    .await;
    assert_eq!(unknown["state"], "unknown");
    assert_eq!(unknown["revision"], 3);
    let late = core
        .operate(
            "peer.task.advance",
            &ctx,
            &json!({"task_id":task_id,"claim_id":"prepared-claim","event_id":"prepared-late",
                    "expected_revision":3,"state":"completed"}),
        )
        .await;
    assert!(late.is_err(), "late completion must not overwrite unknown");
}
