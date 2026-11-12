use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn context(actor: &str, chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain,
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

#[tokio::test]
async fn peer_branch_from_a_revoked_source_is_blocked() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("alice", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("alice", Some(chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"PEER-SECRET"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"peer-revoke"}),
    )
    .await;
    let checkpoint = call(
        &core,
        &ctx,
        "chain.checkpoint",
        json!({"cut_id":published["cut_id"]}),
    )
    .await;
    let socket = call(
        &core,
        &ctx,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],
               "acl":{"message":["alice"],"signal":["alice"],"spawn":["alice"],"read":["alice"]}}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let peer = call(
        &core,
        &ctx,
        "socket.spawn",
        json!({"socket_id":socket,"request_id":"branch-1","message_id":"branch-1",
               "text":"peer branch","lifecycle":"P"}),
    )
    .await;
    let peer_chain = peer["chain_id"].as_str().unwrap().to_owned();
    let peer_ctx = context("alice", Some(peer_chain));

    call(
        &core,
        &ctx,
        "security.classify",
        json!({"source_id":source,"policy":"access_revoke"}),
    )
    .await;
    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":source,"reason":"peer branch test"}),
    )
    .await;

    assert_eq!(
        core.operate("cut.get", &peer_ctx, &json!({}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    let pack = call(&core, &peer_ctx, "experiment.pack", json!({})).await;
    assert_eq!(pack["privacy"]["revoked"], true);
    assert!(pack["blocks"].as_array().unwrap().is_empty());
    let status = call(&core, &peer_ctx, "chain.get", json!({})).await;
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("PEER-SECRET")
    );
    store.close().await;
}
