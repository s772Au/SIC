use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    error::CoreError,
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

async fn probe(
    core: &Core,
    ctx: &OperationContext,
    op: &str,
    payload: Value,
) -> Result<Value, CoreError> {
    core.operate(op, ctx, &payload)
        .await
        .map(|response| response["result"].clone())
}

/// Layout decision 2026-11-13: shared_i is the mainline; the separate layout is
/// retained only as the legacy Python route. This core must never build a shared
/// prefix while reporting "separate", so an explicit request for separate is
/// refused with 501 instead of silently accepted.
#[tokio::test]
async fn shared_i_is_the_default_and_separate_is_refused_instead_of_silently_shared() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store);
    let owner = context("owner", None);
    let chain = call(&core, &owner, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let scoped = context("owner", Some(chain));

    let configuration = call(&core, &scoped, "config.get", json!({})).await;
    assert_eq!(configuration["configuration"]["prefix_layout"], "shared_i");

    let refused = probe(
        &core,
        &scoped,
        "config.update",
        json!({"prefix_layout": "separate"}),
    )
    .await
    .expect_err("separate layout must not be silently accepted");
    assert_eq!(refused.code, "not_implemented");
    assert_eq!(refused.status, 501);
    assert!(
        refused.message.contains("legacy Python route"),
        "refusal must name the retained route, got: {}",
        refused.message
    );

    // The refused update must not have changed the stored configuration.
    let after = call(&core, &scoped, "config.get", json!({})).await;
    assert_eq!(after["configuration"]["prefix_layout"], "shared_i");

    // An unknown value is still a plain validation error, not a 501.
    let invalid = probe(
        &core,
        &scoped,
        "config.update",
        json!({"prefix_layout": "invented"}),
    )
    .await
    .expect_err("unknown layout must be rejected");
    assert_eq!(invalid.code, "invalid");

    // A non-string value is a validation error too.
    let non_text = probe(&core, &scoped, "config.update", json!({"prefix_layout": 3}))
        .await
        .expect_err("non-text layout must be rejected");
    assert_eq!(non_text.code, "invalid");

    // Explicit shared_i stays accepted and is idempotent.
    let accepted = call(
        &core,
        &scoped,
        "config.update",
        json!({"prefix_layout": "shared_i"}),
    )
    .await;
    assert_eq!(accepted["configuration"]["prefix_layout"], "shared_i");
}
