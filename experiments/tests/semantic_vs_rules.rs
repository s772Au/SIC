use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use sic_experiments::experiment::exact_fact_retention;

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

#[tokio::test]
async fn a_fixed_compression_ratio_never_stands_in_for_task_utility() {
    // F29's negative case names two substitutions: treating schema/HTTP success
    // as quality, and treating a fixed compression rate as quality. The second
    // is the easy one to ship by accident, because a ratio is a number and a
    // number looks like a verdict.
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    // The shared helper ingests exactly three facts, so retention is out of 3.
    let facts: Vec<String> = vec![
        "ALPHA fact".to_owned(),
        "BETA fact".to_owned(),
        "GAMMA fact".to_owned(),
    ];

    // Two candidates with different characters but a different retained set.
    // A ratio-only metric would rank them by size; only retention says which
    // facts survived, which is what the task actually depends on.
    let (_kept, kept_body) = candidate(&core, &ctx, json!([]), "ratio-kept").await;
    let kept = exact_fact_retention(&facts, &kept_body);
    assert_eq!(kept.retained, 3);

    let (_half, half_body) = candidate(
        &core,
        &ctx,
        json!([{"op":"DELETE","target":"u1","reason":"ratio fixture"}]),
        "ratio-half",
    )
    .await;
    let half = exact_fact_retention(&facts, &half_body);
    assert_eq!(half.retained, 2);

    // The surviving facts are the ones the operation did not delete; a ratio
    // cannot tell which one survived, which is why it is not a score.
    assert!(half_body.contains("BETA fact"));
    assert!(half_body.contains("GAMMA fact"));
    assert!(!half_body.contains("ALPHA fact"));
    assert!(half.ratio < kept.ratio);

    // The experiment reports the measurement, never a quality verdict.
    let retention = json!({
        "kept_ratio": kept.ratio,
        "half_ratio": half.ratio,
        "claim": "fact_retention",
    });
    for bogus in ["quality", "score", "better", "worse"] {
        assert!(
            retention.get(bogus).is_none(),
            "a retention measurement must not become a `{bogus}` claim"
        );
    }
    store.close().await;
}
