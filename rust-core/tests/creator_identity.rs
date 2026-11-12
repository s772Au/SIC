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
