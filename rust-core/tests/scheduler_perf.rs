//! Cost guard for one scheduler pass.
//!
//! Each `runtime.schedule` pass re-reads `chain.get` at several decision points.
//! This measures how much that actually costs so a later optimization is driven
//! by a number rather than by an assumption.
//!
//! cargo test --release --test scheduler_perf -- --ignored --nocapture

use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::time::Instant;

fn context(actor: &str, chain: &str) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: Some(chain.to_owned()),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|e| panic!("{op} failed: {e:?}"))["result"]
        .clone()
}

fn policy() -> Value {
    json!({
        "context_id": "main", "min_sources_per_wb": 2, "max_sources_per_wb": 4,
        "max_units_per_wb": 512, "level_batch_sizes": {}, "max_level": 1,
        "flush": false, "max_actions": 128, "max_model_calls": 32,
        "editor_mode": "program_fixture", "program_rules": ["deduplicate_exact"]
    })
}

#[tokio::test]
#[ignore = "perf measurement, not a correctness gate"]
async fn a_single_pending_source_costs_one_wait_cycle() {
    let core = Core::new(Store::memory().await.unwrap());
    let created = call(&core, &context("owner", ""), "chain.create", json!({})).await;
    let chain = created["chain_id"].as_str().unwrap().to_owned();
    let ctx = context("owner", &chain);
    call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"only"}]}}),
    )
    .await;

    const RUNS: usize = 200;
    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..RUNS {
        let r = call(&core, &ctx, "runtime.schedule", policy()).await;
        sink += r["published"].as_array().unwrap().len();
    }
    let elapsed = start.elapsed();
    println!("schedule pass runs={RUNS} total={:?} per-pass={:?} sink={sink}", elapsed, elapsed / RUNS as u32);
    let last = call(&core, &ctx, "runtime.schedule", policy()).await;
    println!("operations this pass: {:?}", last["operations"]);
    let mut by_op: std::collections::BTreeMap<String, usize> = Default::default();
    for op in last["operations"].as_array().unwrap_or(&vec![]) {
        *by_op.entry(op.as_str().unwrap_or("?").to_owned()).or_default() += 1;
    }
    println!("per-op counts: {:?}", by_op);
    let snap = call(&core, &ctx, "chain.get", json!({})).await;
    println!("chain.get keys: {:?}", snap.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
    println!("  cut_seq={:?}", snap.get("cut_seq"));
    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    println!("cut.get keys: {:?}", cut.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));

    // Isolate the snapshot read the pass repeats.
    let start = Instant::now();
    for _ in 0..RUNS {
        call(&core, &ctx, "chain.get", json!({})).await;
    }
    println!("chain.get  runs={RUNS} total={:?} per-call={:?}", start.elapsed(), start.elapsed() / RUNS as u32);
}
