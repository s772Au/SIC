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
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}
async fn ingest(core: &Core, ctx: &OperationContext, text: &str) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn prepare(core: &Core, ctx: &OperationContext, source: &str) -> Value {
    call(
        core,
        ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await
}
async fn publish(core: &Core, ctx: &OperationContext, job: &str, key: &str) -> String {
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn late_material_waits_for_the_next_block_and_is_not_in_the_frozen_cb() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));

    let early = ingest(&core, &ctx, "early fact").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[early]})).await;
    let prepared = prepare(&core, &ctx, &early).await;
    let job = prepared["job_id"].as_str().unwrap().to_owned();
    assert_eq!(prepared["editable_units"].as_array().unwrap().len(), 1);

    // Material that arrives after the Editor's WB was frozen stays pending.
    let late = ingest(&core, &ctx, "late fact").await;
    let view = call(&core, &ctx, "cut.get", json!({})).await;
    assert!(
        view["pending"]
            .as_array()
            .is_some_and(|pending| pending.iter().any(|id| id == &late)),
        "late material must be pending, not part of the frozen snapshot: {view:?}"
    );

    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":"early edited"}],
               "pvi":{}}),
    )
    .await;
    let first_cb = publish(&core, &ctx, &job, "late-block-one").await;
    let body = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"][0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(body.contains("early edited"), "{body}");
    assert!(
        !body.contains("late fact"),
        "the frozen CB must not absorb later material: {body}"
    );

    // The next block forms its own WB and CB; the late material is preserved.
    call(&core, &ctx, "wb.form", json!({"source_ids":[late]})).await;
    let next = prepare(&core, &ctx, &late).await;
    let next_job = next["job_id"].as_str().unwrap().to_owned();
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":next_job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":"late edited"}],
               "pvi":{}}),
    )
    .await;
    let second_cb = publish(&core, &ctx, &next_job, "late-block-two").await;
    assert_ne!(first_cb, second_cb);
    let cbs = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(cbs.len(), 2);
    assert!(cbs[1]["body"].as_str().unwrap().contains("late edited"));

    // Provenance stays exact: CB1 expands only to the early source.
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":first_cb})).await;
    assert_eq!(expanded["source_ids"], json!([early]));
    assert_eq!(expanded["original_units"].as_array().unwrap().len(), 1);
    assert_eq!(expanded["original_units"][0]["text"], "early fact");
    core.store.close().await;
}
