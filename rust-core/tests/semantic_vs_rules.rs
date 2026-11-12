use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    experiment::exact_fact_retention,
    storage::Store,
};

fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
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
async fn candidate(core: &Core, ctx: &OperationContext, ops: Value, key: &str) -> (Value, String) {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"user","content":"ALPHA fact"},
            {"role":"user","content":"BETA fact"},
            {"role":"user","content":"GAMMA fact"}
        ]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let units: Vec<String> =
        call(core, ctx, "source.read", json!({"source_id":source})).await["source"]["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
            .collect();
    let job = call(
        core,
        ctx,
        "edit.prepare",
        json!({"source_id":source,
               "assignments":[{"editor":"e1","unit_ids":units}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = core
        .operate(
            "edit.submit",
            ctx,
            &json!({"job_id":job,"editor":"e1","ops":ops,"pvi":{}}),
        )
        .await
        .unwrap();
    assert_eq!(run["result"]["status"], "ready");
    let state = call(core, ctx, "chain.get", json!({})).await;
    let published = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await;
    let body = call(core, ctx, "cut.get", json!({})).await["view"]["cbs"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    (published, body)
}

#[tokio::test]
async fn mechanical_validity_does_not_imply_fact_preservation() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let facts = vec![
        "ALPHA fact".to_owned(),
        "BETA fact".to_owned(),
        "GAMMA fact".to_owned(),
    ];

    // Rule-valid but lossy: DELETE is a valid operation.
    let (_lossy_cut, lossy_body) = candidate(
        &core,
        &ctx,
        json!([{"op":"DELETE","target":"u2","reason":"simulated loss"}]),
        "lossy",
    )
    .await;
    let lossy = exact_fact_retention(&facts, &lossy_body);
    assert_eq!(lossy.retained, 2);
    assert!((lossy.ratio - 2.0 / 3.0).abs() < 1e-9);
    assert!(!lossy_body.contains("BETA fact"));

    // Rule-valid and faithful: empty ops mean default KEEP.
    let (_faithful_cut, faithful_body) = candidate(&core, &ctx, json!([]), "faithful").await;
    let faithful = exact_fact_retention(&facts, &faithful_body);
    assert_eq!(faithful.retained, 3);
    assert_eq!(faithful.ratio, 1.0);

    // Both candidates passed the same mechanical validation; only the
    // independent fact-retention check separates them. This is not a semantic
    // quality score.
    assert!(lossy.ratio < faithful.ratio);
    store.close().await;
}
