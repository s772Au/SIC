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
async fn edit_block(
    core: &Core,
    ctx: &OperationContext,
    text: &str,
    ops: Value,
    key: &str,
) -> (String, String) {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        core,
        ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        core,
        ctx,
        "edit.submit",
        json!({"job_id":job,"editor":"e1","ops":ops,"pvi":{}}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    let cb = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source, cb)
}

#[tokio::test]
async fn three_independent_measurements_are_reported_without_quality_claims() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let (source, _) = edit_block(
        &core,
        &ctx,
        "alpha unit",
        json!([{"op":"REPLACE","target":"u1","content":"alpha edited"}]),
        "measure-one",
    )
    .await;
    edit_block(
        &core,
        &ctx,
        "beta unit",
        json!([{"op":"DELETE","target":"u1","reason":"measurement fixture"}]),
        "measure-two",
    )
    .await;

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let measurements = &accounting["measurements"];
    assert_eq!(measurements["class"], "host_measured_per_chain");
    assert_eq!(measurements["blocks"]["count"], 2);
    assert_eq!(measurements["blocks"]["body_chars"]["count"], 2);
    assert!(
        measurements["blocks"]["body_chars"]["max"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(measurements["blocks"]["truncated"], false);
    assert!(measurements["edit_noise"]["ops"].as_u64().unwrap() >= 2);
    assert!(
        measurements["edit_noise"]["by_kind"]["REPLACE"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(
        measurements["edit_noise"]["by_kind"]["DELETE"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(
        measurements["edit_noise"]["ops_per_block"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(measurements["cost"]["storage_bytes"].as_u64().unwrap() > 0);
    let non_claims = measurements["non_claims"].as_array().unwrap();
    assert!(non_claims.contains(&json!("experiment_not_run")));
    assert!(non_claims.contains(&json!("provider_quality_not_measured")));

    // Revocation suppresses content-derived measurements but keeps the ledger
    // readable; no phantom zero-block claim is produced.
    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":source,"reason":"measurement revoke"}),
    )
    .await;
    let after = call(&core, &ctx, "accounting.get", json!({})).await;
    assert_eq!(after["measurements"]["suppressed"], "revocation");
    assert_eq!(after["resources"]["storage"]["class"], "host_measured");
}
