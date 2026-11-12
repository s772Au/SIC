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
        allow_revoke: true,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn make_chain(core: &Core, actor: &str) -> String {
    call(core, &context(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn publish(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> Value {
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
        json!({"source_id":source["source_id"],"rules":[]}),
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
async fn historical_checkpoint_and_replay_branches_are_blocked_after_revocation() {
    const SENTINEL: &str = "BRANCH-SECRET-XYZ";
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let alice_id = make_chain(&core, "alice").await;
    let _bob_id = make_chain(&core, "bob").await;
    let alice = context("alice", Some(&alice_id));
    let published = publish(&core, &alice, &format!("{SENTINEL} body"), "branch-cb").await;
    let cb = published["cb_id"].as_str().unwrap().to_owned();
    let secret = published["source_ids"][0].as_str().unwrap().to_owned();
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
            "message":[],"signal":[],"spawn":[],"read":[alice_id]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Both the frozen checkpoint and the replay are readable before revoke.
    let page = call(
        &core,
        &alice,
        "socket.read",
        json!({"socket_id":socket,"offset":0,"limit":100}),
    )
    .await;
    assert!(page["value"].as_str().unwrap().contains(SENTINEL));
    let replayed = call(&core, &alice, "memory.replay", json!({"cb_id":cb})).await;
    assert!(replayed["body"].as_str().unwrap().contains(SENTINEL));

    call(
        &core,
        &alice,
        "security.revoke",
        json!({"source_id":secret,"reason":"branch revoke"}),
    )
    .await;

    for (op, payload) in [
        (
            "socket.read",
            json!({"socket_id":socket,"offset":0,"limit":100}),
        ),
        ("memory.replay", json!({"cb_id":cb})),
        ("hierarchy.timeline", json!({"level":1,"kind":"cb"})),
        ("cut.get", json!({})),
        ("chain.checkpoint", json!({"cut_id":published["cut_id"]})),
        ("source.read", json!({"source_id":secret})),
    ] {
        let error = core.operate(op, &alice, &payload).await.unwrap_err();
        assert_eq!(error.code, "forbidden", "{op}");
        assert!(!error.message.contains(SENTINEL));
    }
    // Route metadata stays available and never embeds the material.
    let routes = call(&core, &alice, "chain.list", json!({})).await;
    let routes_text = serde_json::to_string(&routes).unwrap();
    assert!(!routes_text.contains(SENTINEL), "{routes_text}");
    // The checkpoint object and source bytes remain; only authorized reads are blocked.
    let mut tx = store.begin().await.unwrap();
    let objects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE id IN (?,?)")
        .bind(&cb)
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(objects, 2);
}
