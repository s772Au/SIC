use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use sqlx::Row;

fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
        chain_id: chain,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}
async fn stored_object(store: &Store, id: &str) -> (String, String) {
    let mut tx = store.begin().await.unwrap();
    let row = sqlx::query("SELECT data,digest FROM objects WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let data: String = row.try_get("data").unwrap();
    let digest: String = row.try_get("digest").unwrap();
    tx.commit().await.unwrap();
    (data, digest)
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn publish_level_one(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> String {
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
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn form_and_publish(
    core: &Core,
    ctx: &OperationContext,
    level: u32,
    cb_ids: Vec<String>,
    key: &str,
) -> String {
    let higher = call(
        core,
        ctx,
        "hierarchy.form",
        json!({"level":level,"cb_ids":cb_ids}),
    )
    .await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":higher["source_id"],"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn correction_enters_a_new_higher_order_block_without_rewriting_old_cb() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let first = publish_level_one(&core, &ctx, "OLD-FACT-A", "corr-a").await;
    let second = publish_level_one(&core, &ctx, "OLD-FACT-B", "corr-b").await;
    let old_l2 = form_and_publish(
        &core,
        &ctx,
        2,
        vec![first.clone(), second.clone()],
        "corr-l2",
    )
    .await;
    let old_l2_stored_before = stored_object(&store, &old_l2).await;
    let old_l2_before = call(&core, &ctx, "memory.replay", json!({"cb_id":old_l2})).await["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(old_l2_before.contains("OLD-FACT-A"));
    assert!(old_l2_before.contains("OLD-FACT-B"));

    let correction = publish_level_one(&core, &ctx, "CORRECTION-NEW-FACT", "corr-c").await;
    let correction_l2 = form_and_publish(&core, &ctx, 2, vec![correction], "corr-l2c").await;
    let new_l3 = form_and_publish(
        &core,
        &ctx,
        3,
        vec![old_l2.clone(), correction_l2],
        "corr-l3",
    )
    .await;

    let old_l2_after = call(&core, &ctx, "memory.replay", json!({"cb_id":old_l2})).await["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        old_l2_before, old_l2_after,
        "old CB body must stay identical"
    );
    assert_eq!(
        old_l2_stored_before,
        stored_object(&store, &old_l2).await,
        "old CB stored bytes/digest must stay byte-identical"
    );
    assert!(!old_l2_after.contains("CORRECTION-NEW-FACT"));
    let l3_body = call(&core, &ctx, "memory.replay", json!({"cb_id":new_l3})).await["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(l3_body.contains("OLD-FACT-A"));
    assert!(l3_body.contains("CORRECTION-NEW-FACT"));
    let hits = call(
        &core,
        &ctx,
        "memory.search",
        json!({"query":"CORRECTION-NEW-FACT","scope":"cb","level":3}),
    )
    .await;
    assert_eq!(hits["hits"][0]["cb_id"], new_l3);
    store.close().await;
}
