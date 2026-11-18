//! A few operations refuse a bare call for reasons a field list cannot express.
//!
//! These are the cases a host actually trips over: the operation looks like it
//! should take an argument, and the refusal does not say which rule was broken.
//! The tests here pin the wording so a later edit does not silently make it
//! vague again, and pin the alternative forms so the tables stay truthful.

use serde_json::json;
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn context(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

async fn call(
    core: &Core,
    ctx: &OperationContext,
    op: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}

#[tokio::test]
async fn worker_run_says_it_takes_no_payload_rather_than_unknown_argument() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await;
    let chain = chain["chain_id"].as_str().unwrap().to_owned();
    let ctx = context("owner", Some(&chain));

    // The mistake a host makes: assuming the Worker needs a source_id because
    // every other editing operation does.
    let error = core
        .operate("worker.run", &ctx, &json!({"source_id": "source_x"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("accepts no payload"),
        "the refusal must name the rule, not just call the field unknown: {}",
        error.message
    );
    assert!(
        error.message.contains("retry_after_attempt"),
        "the refusal must name what is accepted: {}",
        error.message
    );
    assert!(
        error.message.contains("source_id"),
        "the refusal should echo what was sent, so the caller sees the mismatch: {}",
        error.message
    );
}

#[tokio::test]
async fn runtime_budget_refuses_a_call_that_names_neither_grant_nor_permit() {
    // A single field cannot express "grant OR consume". The handler says so,
    // and the payload table records it in notes rather than inventing a field.
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await;
    let chain = chain["chain_id"].as_str().unwrap().to_owned();
    let ctx = context("owner", Some(&chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"unit"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let error = core
        .operate(
            "runtime.budget",
            &ctx,
            &json!({"wb_source_id": source}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    let lowered = error.message.to_lowercase();
    assert!(
        lowered.contains("budget") && lowered.contains("permit_id"),
        "the refusal must name both alternatives: {}",
        error.message
    );
}

#[tokio::test]
async fn hierarchy_expand_refuses_two_targets_instead_of_guessing_one() {
    // cb_id and wb_id are alternatives, not a pair. Accepting both and
    // preferring one would silently discard the caller's intent.
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await;
    let chain = chain["chain_id"].as_str().unwrap().to_owned();
    let ctx = context("owner", Some(&chain));

    let error = core
        .operate(
            "hierarchy.expand",
            &ctx,
            &json!({"cb_id": "cb_x", "wb_id": "wb_y"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("exactly one"),
        "the refusal must say the fields are alternatives: {}",
        error.message
    );
}