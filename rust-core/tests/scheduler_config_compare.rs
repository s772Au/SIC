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
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn chain_with_sources(core: &Core, actor: &str, count: usize) -> (String, OperationContext) {
    let chain = call(core, &context(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(actor, Some(&chain));
    for n in 0..count {
        call(
            core,
            &ctx,
            "source.ingest",
            json!({"request":{"messages":[{"role":"user","content":format!("source {n}")}]}}),
        )
        .await;
    }
    (chain, ctx)
}
fn policy(max_sources: usize) -> Value {
    json!({
        "context_id": "main",
        "min_sources_per_wb": 1,
        "max_sources_per_wb": max_sources,
        "max_units_per_wb": 512,
        "level_batch_sizes": {},
        "max_level": 1,
        "flush": false,
        "max_actions": 128,
        "max_model_calls": 32,
        "editor_mode": "program_fixture",
        "program_rules": ["deduplicate_exact"]
    })
}

#[tokio::test]
async fn block_length_configuration_changes_published_blocks_and_measurements() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let (_small_chain, small_ctx) = chain_with_sources(&core, "small", 6).await;
    let small = call(&core, &small_ctx, "runtime.schedule", policy(2)).await;
    assert_eq!(small["published"].as_array().unwrap().len(), 3);
    let small_measurements =
        call(&core, &small_ctx, "accounting.get", json!({})).await["measurements"].clone();
    assert_eq!(small_measurements["blocks"]["count"], 3);

    let (_large_chain, large_ctx) = chain_with_sources(&core, "large", 6).await;
    let large = call(&core, &large_ctx, "runtime.schedule", policy(6)).await;
    assert_eq!(large["published"].as_array().unwrap().len(), 1);
    let large_measurements =
        call(&core, &large_ctx, "accounting.get", json!({})).await["measurements"].clone();
    assert_eq!(large_measurements["blocks"]["count"], 1);

    // The measurements expose the block-length difference without claiming a
    // model-quality or provider-bill result.
    assert_ne!(
        small_measurements["blocks"]["body_chars"]["avg"],
        large_measurements["blocks"]["body_chars"]["avg"]
    );
    assert_eq!(
        small_measurements["non_claims"],
        json!([
            "provider_quality_not_measured",
            "experiment_not_run",
            "real_bill_not_available"
        ])
    );
    assert_eq!(large_measurements["blocks"]["body_chars"]["count"], 1);
    store.close().await;
}

fn level_policy(batch: usize) -> Value {
    json!({
        "context_id": "main",
        "min_sources_per_wb": 1,
        "max_sources_per_wb": 1,
        "max_units_per_wb": 512,
        "level_batch_sizes": {"2": batch},
        "max_level": 2,
        "flush": false,
        "max_actions": 128,
        "max_model_calls": 32,
        "editor_mode": "program_fixture",
        "program_rules": ["deduplicate_exact"]
    })
}

#[tokio::test]
async fn level_batch_configuration_changes_higher_block_counts() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let (_pair_chain, pair_ctx) = chain_with_sources(&core, "pair", 4).await;
    let pair = call(&core, &pair_ctx, "runtime.schedule", level_policy(2)).await;
    assert_eq!(pair["published"].as_array().unwrap().len(), 6);
    assert_eq!(
        pair["published"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["level"] == 2)
            .count(),
        2
    );
    let pair_measurements =
        call(&core, &pair_ctx, "accounting.get", json!({})).await["measurements"].clone();
    assert_eq!(pair_measurements["blocks"]["count"], 6);

    let (_all_chain, all_ctx) = chain_with_sources(&core, "all", 4).await;
    let all = call(&core, &all_ctx, "runtime.schedule", level_policy(4)).await;
    assert_eq!(all["published"].as_array().unwrap().len(), 5);
    assert_eq!(
        all["published"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["level"] == 2)
            .count(),
        1
    );
    let all_measurements =
        call(&core, &all_ctx, "accounting.get", json!({})).await["measurements"].clone();
    assert_eq!(all_measurements["blocks"]["count"], 5);
    store.close().await;
}

#[tokio::test]
async fn level_batch_wait_declaration_names_the_remaining_interval() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let (_chain, ctx) = chain_with_sources(&core, "wait", 3).await;
    let report = call(&core, &ctx, "runtime.schedule", level_policy(2)).await;
    assert_eq!(report["published"].as_array().unwrap().len(), 4);
    assert_eq!(report["needs_flush"], true);
    assert_eq!(report["wait"]["code"], "level_batch");
    assert_eq!(report["wait"]["level"], 2);
    assert_eq!(report["wait"]["available"], 3);
    assert_eq!(report["wait"]["required"], 2);
    assert!(
        report["wait"]["required_action"]
            .as_str()
            .unwrap()
            .contains("flush")
    );
    store.close().await;
}
