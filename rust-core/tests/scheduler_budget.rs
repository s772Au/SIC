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
fn policy() -> Value {
    json!({
        "context_id": "main",
        "min_sources_per_wb": 2,
        "max_sources_per_wb": 4,
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
async fn pending_root(core: &Core, ctx: &OperationContext) -> String {
    call(core, ctx, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]["pending"][0]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn per_wb_budget_permits_one_early_block_and_then_waits() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let _first = ingest(&core, &ctx, "only source").await;

    let waiting = call(&core, &ctx, "runtime.schedule", policy()).await;
    assert!(waiting["published"].as_array().unwrap().is_empty());
    assert_eq!(waiting["needs_flush"], true);
    assert!(
        waiting["wait_reason"]
            .as_str()
            .unwrap()
            .contains("min_sources")
    );
    assert_eq!(waiting["wait"]["code"], "interleave_budget_missing");
    assert_eq!(waiting["wait"]["level"], 1);
    assert_eq!(waiting["wait"]["required"], 2);

    let root = pending_root(&core, &ctx).await;
    let ledger = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":root,"budget":1}),
    )
    .await;
    assert_eq!(ledger["budget"], 1);
    assert_eq!(ledger["remaining"], 1);

    let scheduled = call(&core, &ctx, "runtime.schedule", policy()).await;
    assert_eq!(scheduled["published"].as_array().unwrap().len(), 1);
    let interleaves = scheduled["interleaves"].as_array().unwrap();
    assert_eq!(interleaves.len(), 1);
    assert_eq!(interleaves[0]["cost"], 1);
    assert_eq!(interleaves[0]["remaining"], 0);
    assert_eq!(interleaves[0]["wb_source_id"], root);
    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    assert_eq!(accounting["measurements"]["blocks"]["count"], 1);

    // A new WB has no budget: the scheduler waits instead of interleaving.
    let _second = ingest(&core, &ctx, "second source").await;
    let second_root = pending_root(&core, &ctx).await;
    let no_budget = call(&core, &ctx, "runtime.schedule", policy()).await;
    assert!(no_budget["published"].as_array().unwrap().is_empty());
    assert!(
        no_budget["wait_reason"]
            .as_str()
            .unwrap()
            .contains("budget_missing"),
        "{no_budget}"
    );
    assert_eq!(no_budget["wait"]["code"], "interleave_budget_missing");

    // Exhaust the second WB with one explicit permit; the scheduler then
    // waits instead of over-spending, and the permit replay is free.
    call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":second_root,"budget":1}),
    )
    .await;
    let consumed = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":second_root,"permit_id":"manual","cost":1}),
    )
    .await;
    assert_eq!(consumed["consumed"], true);
    assert_eq!(consumed["remaining"], 0);
    let replay = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":second_root,"permit_id":"manual","cost":1}),
    )
    .await;
    assert_eq!(replay["consumed"], false);
    assert_eq!(replay["replayed"], true);

    // Validation: unknown targets, invalid amounts, and shrinking below the
    // consumed amount are rejected rather than silently accepted.
    assert_eq!(
        core.operate(
            "runtime.budget",
            &ctx,
            &json!({"wb_source_id":"source_missing","budget":1})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
    assert_eq!(
        core.operate(
            "runtime.budget",
            &ctx,
            &json!({"wb_source_id":second_root,"budget":0})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    assert_eq!(
        core.operate(
            "runtime.budget",
            &ctx,
            &json!({"wb_source_id":second_root,"permit_id":"too-costly","cost":9})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":second_root,"budget":2}),
    )
    .await;
    call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":second_root,"permit_id":"extra","cost":1}),
    )
    .await;
    assert_eq!(
        core.operate(
            "runtime.budget",
            &ctx,
            &json!({"wb_source_id":second_root,"budget":1})
        )
        .await
        .unwrap_err()
        .code,
        "conflict"
    );
    let exhausted = call(&core, &ctx, "runtime.schedule", policy()).await;
    assert!(exhausted["published"].as_array().unwrap().is_empty());
    assert!(
        exhausted["wait_reason"]
            .as_str()
            .unwrap()
            .contains("budget_exhausted"),
        "{exhausted}"
    );
    assert_eq!(exhausted["wait"]["code"], "interleave_budget_exhausted");
}
