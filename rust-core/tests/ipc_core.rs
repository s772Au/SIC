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
async fn call(core: &Core, ctx: &OperationContext, op: &str, p: Value) -> Value {
    core.operate(op, ctx, &p).await.unwrap()["result"].clone()
}
async fn make(core: &Core, actor: &str) -> String {
    call(core, &context(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn source(core: &Core, ctx: &OperationContext, text: &str) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn publish(core: &Core, ctx: &OperationContext, source_id: &str, key: &str) -> Value {
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source_id,"rules":["drop_empty"]}),
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

#[tokio::test]
async fn historic_peer_and_communications_are_transactional_and_replayable() {
    let core = Core::new(Store::memory().await.unwrap());
    let a = make(&core, "alice").await;
    let b = make(&core, "bob").await;
    let c = make(&core, "charlie").await;
    let alice = context("alice", Some(&a));
    let bob = context("bob", Some(&b));
    let charlie = context("charlie", Some(&c));
    let initial = source(&core, &alice, "first").await;
    let first = publish(&core, &alice, &initial, "first").await;
    let checkpoint = call(
        &core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":first["cut_id"]}),
    )
    .await;
    let socket = call(
        &core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
        "message":[b],"signal":[b],"spawn":[b],"read":[b]}}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let later = source(&core, &alice, "second").await;
    publish(&core, &alice, &later, "second").await;
    let old_page = call(
        &core,
        &bob,
        "socket.read",
        json!({"socket_id":socket,"offset":0,"limit":3}),
    )
    .await;
    assert_eq!(old_page["value"], "fir");
    assert_eq!(old_page["next_offset"], 3);
    assert_eq!(old_page["cb_ids"].as_array().unwrap().len(), 1);
    assert!(
        core.operate("socket.read", &charlie, &json!({"socket_id":socket}))
            .await
            .is_err()
    );
    assert!(core.operate("socket.message",&charlie,&json!({"socket_id":socket,"request_id":"q","message_id":"m","text":"bad","lifecycle":"P"})).await.is_err());
    let persistent = json!({"socket_id":socket,"request_id":"q1","message_id":"p","text":"persist clue","lifecycle":"P"});
    call(&core, &bob, "socket.message", persistent.clone()).await;
    assert_eq!(
        core.operate("socket.message", &bob, &persistent)
            .await
            .unwrap()["evidence"]["replayed"],
        true
    );
    call(&core,&bob,"socket.message",json!({"socket_id":socket,"request_id":"q2","message_id":"v","text":"volatile clue","lifecycle":"V"})).await;
    let trigger = source(&core, &alice, "trigger").await;
    let published = publish(&core, &alice, &trigger, "with-messages").await;
    let view = call(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(view["view"]["cbs"][2]["body"], "trigger\npersist clue");
    assert_eq!(view["pending"].as_array().unwrap().len(), 1);
    let replay = call(
        &core,
        &alice,
        "memory.replay",
        json!({"cb_id":published["cb_id"]}),
    )
    .await;
    assert_eq!(replay["body"], "trigger\npersist clue");
    let volatile = published["volatile_source_id"].as_str().unwrap();
    let volatile_material = call(&core, &alice, "source.read", json!({"source_id":volatile})).await;
    assert_eq!(
        volatile_material["source"]["units"][0]["text"],
        "volatile clue"
    );
    let inbox = call(&core, &alice, "socket.inbox", json!({})).await;
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["consumed_by_cut"].is_string())
    );
    let spawn = json!({"socket_id":socket,"request_id":"spawn1","message_id":"task1","text":"peer task","lifecycle":"P"});
    let peer = call(&core, &bob, "socket.spawn", spawn.clone()).await;
    let repeated = core.operate("socket.spawn", &bob, &spawn).await.unwrap();
    assert_eq!(repeated["evidence"]["replayed"], true);
    assert_eq!(repeated["result"], peer);
    let peer_id = peer["chain_id"].as_str().unwrap();
    let sibling = context("bob", Some(peer_id));
    let peer_view = call(&core, &sibling, "cut.get", json!({})).await;
    assert_eq!(peer_view["view"]["cbs"].as_array().unwrap().len(), 1);
    assert_eq!(peer_view["view"]["cbs"][0]["body"], "first");
    let pending = peer_view["pending"][0].as_str().unwrap();
    publish(&core, &sibling, pending, "peer-task").await;
    let peer_view = call(&core, &sibling, "cut.get", json!({})).await;
    assert_eq!(peer_view["view"]["cbs"][1]["body"], "peer task");
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,"signal_id":"pause1","action":"pause"}),
    )
    .await;
    assert_eq!(
        call(&core, &alice, "chain.get", json!({})).await["run_state"],
        "waiting"
    );
    assert!(
        core.operate(
            "edit.program",
            &alice,
            &json!({"source_id":volatile,"rules":[]})
        )
        .await
        .is_err()
    );
    call(
        &core,
        &bob,
        "socket.signal",
        json!({"socket_id":socket,"signal_id":"resume1","action":"resume"}),
    )
    .await;
    publish(&core, &alice, volatile, "finish-volatile").await;
    assert!(
        core.operate(
            "socket.acl.update",
            &bob,
            &json!({"socket_id":socket,
        "acl":{"message":[],"signal":[],"spawn":[],"read":[]}})
        )
        .await
        .is_err()
    );
    call(
        &core,
        &alice,
        "socket.acl.update",
        json!({"socket_id":socket,
        "acl":{"message":[],"signal":[],"spawn":[],"read":[]}}),
    )
    .await;
    assert!(
        core.operate("socket.read", &bob, &json!({"socket_id":socket}))
            .await
            .is_err()
    );
    assert!(
        core.operate(
            "socket.message",
            &bob,
            &json!({"socket_id":socket,
        "request_id":"revoked","message_id":"revoked","text":"no","lifecycle":"V"})
        )
        .await
        .is_err()
    );
}
