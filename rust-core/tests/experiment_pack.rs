use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

const SENTINEL: &str = "PACK-SECRET-42";

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
async fn stored_digest(core: &Core, id: &str) -> String {
    let mut tx = core.store.begin().await.unwrap();
    let digest: String = sqlx::query_scalar("SELECT digest FROM objects WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    digest
}

#[tokio::test]
async fn experiment_pack_keeps_digests_not_material_and_hides_revoked_data() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":format!("{SENTINEL} body")}]}}),
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
    let cb = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"pack-cb"}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(pack["format"], "sic-experiment-pack-v1");
    assert_eq!(pack["privacy"]["source_text_included"], false);
    assert_eq!(pack["privacy"]["provider_bill_included"], false);
    assert_eq!(pack["privacy"]["revoked"], false);
    let text = serde_json::to_string(&pack).unwrap();
    assert!(
        !text.contains(SENTINEL),
        "the pack must not contain source text"
    );
    assert_eq!(pack["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        pack["sources"][0]["digest"],
        stored_digest(&core, &source).await
    );
    assert_eq!(pack["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(pack["blocks"][0]["digest"], stored_digest(&core, &cb).await);
    assert!(
        pack["accounting"]["resources"]["storage"]["bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        pack["verification"]["not_included"]
            .as_array()
            .unwrap()
            .contains(&json!("source text"))
    );

    // A later pack keeps the same object digests.
    let again = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(again["sources"][0]["digest"], pack["sources"][0]["digest"]);

    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":source,"reason":"pack test revoke"}),
    )
    .await;
    let revoked = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(revoked["privacy"]["revoked"], true);
    assert!(revoked["sources"].as_array().unwrap().is_empty());
    assert!(revoked["blocks"].as_array().unwrap().is_empty());
    assert_eq!(
        revoked["accounting"]["measurements"]["suppressed"],
        "revocation"
    );
    assert!(!serde_json::to_string(&revoked).unwrap().contains(SENTINEL));
}
