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
        allow_revoke: false,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}

#[tokio::test]
async fn multi_creator_chain_requires_owned_creators_and_does_not_grant_ownership() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let alice = context("alice", None);
    let bob = context("bob", None);
    let first = call(&core, &alice, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = call(&core, &alice, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let child = call(
        &core,
        &alice,
        "chain.create",
        json!({"creators":[first,second]}),
    )
    .await;
    assert_eq!(child["creators"], json!([first, second]));
    let child_id = child["chain_id"].as_str().unwrap().to_owned();

    assert_eq!(
        core.operate(
            "chain.create",
            &alice,
            &json!({"creators":[first,"chain_missing"]})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    let bob_chain = call(&core, &bob, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        core.operate(
            "chain.create",
            &alice,
            &json!({"creators":[first,bob_chain]})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );

    // The creator relation is provenance, not ownership.
    assert_eq!(
        core.operate(
            "chain.get",
            &context("bob", Some(child_id.clone())),
            &json!({})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
    let owner_view = call(
        &core,
        &context(
            "alice",
            Some(child["chain_id"].as_str().unwrap().to_owned()),
        ),
        "chain.get",
        json!({}),
    )
    .await;
    assert_eq!(owner_view["chain_id"], child_id);
    store.close().await;
}


#[tokio::test]
async fn fork_records_its_origin_and_is_not_a_creator_child() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let alice = context("alice", None);
    let base = call(&core, &alice, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let base_ctx = context("alice", Some(base.clone()));

    // Publish one cut so the fork has a real checkpoint to leave from.
    let source = call(
        &core,
        &base_ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"origin material"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // A first-order block is published by preparing it against the pending
    // source and then committing the resulting cut.
    let prepared = call(
        &core,
        &base_ctx,
        "edit.program",
        json!({"source_id":source,"rules":[],"pvi":{}}),
    )
    .await;
    let state = call(&core, &base_ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &base_ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"base-1"}),
    )
    .await;
    let cb_id = published["cb_id"].as_str().unwrap().to_owned();
    let cut = call(&core, &base_ctx, "cut.get", json!({})).await;
    let cut_id = cut["cut_id"].as_str().unwrap().to_owned();
    let checkpoint = call(&core, &base_ctx, "chain.checkpoint", json!({"cut_id":cut_id})).await;
    assert!(checkpoint["checkpoint_id"].is_string());

    // A fork names the route it left from. It is not a creator child, and the
    // origin must stay queryable rather than being flattened into a parent.
    let fork_ctx = context("alice", Some(base.clone()));
    let forked = call(
        &core,
        &fork_ctx,
        "signal.spawn",
        json!({"child_route_id":"forked-child","origin_route_id":base,"prefix_cb_ids":[cb_id],"co_parents":[]}),
    )
    .await;
    assert_eq!(forked["origin"]["kind"], "forked");
    assert_eq!(forked["origin"]["origin_route_id"], base);
    assert!(forked["origin"].get("creators").is_none());
    store.close().await;
}
