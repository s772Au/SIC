//! Cost guard for the shared-prefix assembly hot path.
//!
//! Every provider request re-validates the frozen plan three times and
//! re-serializes the same common tools. This measures that path so a future
//! change that reintroduces per-call reconstruction is visible, not silent.
//!
//! cargo test --release --test prefix_perf -- --ignored --nocapture

use serde_json::json;
use sic_core_rust::prefix::{PrefixPlan, PrefixPlanSpec, RenderedCb};
use std::time::Instant;

fn spec() -> PrefixPlanSpec {
    PrefixPlanSpec {
        model: "test-model".into(),
        provider_id: "test-provider".into(),
        cache_namespace: "test-ns".into(),
        chain_id: "chain_test".into(),
        context_id: "main".into(),
        level: 1,
        cut_id: "cut_test".into(),
        cb_chain: vec![RenderedCb {
            cb_id: "cb_test".into(),
            messages: vec![json!({"role": "assistant", "content": "frozen block"})],
        }],
        projection_scope: "selected-shared-cb-view".into(),
        projection_budget: 1000,
        schema_version: "1".into(),
        tool_version: sic_core_rust::prefix::COMMON_TOOL_VERSION.into(),
    }
}

#[test]
#[ignore = "perf measurement, not a correctness gate"]
fn common_prefix_assembly_stays_cheap() {
    const RUNS: usize = 5000;
    let plan = PrefixPlan::freeze_with_instructions(spec(), vec![]).unwrap();

    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..RUNS {
        sink += plan.common_tools().unwrap().len();
        sink += plan.common_prefix_bytes().unwrap().len();
        sink += plan.digest().len();
    }
    let elapsed = start.elapsed();
    println!(
        "runs={} total={:?} per-run={:?} sink={}",
        RUNS,
        elapsed,
        elapsed / RUNS as u32,
        sink
    );
    assert!(sink > 0);
}

#[test]
#[ignore = "perf measurement, not a correctness gate"]
fn freeze_is_dominated_by_digest_not_by_cloning() {
    const RUNS: usize = 500;
    let start = Instant::now();
    for _ in 0..RUNS {
        let _ = PrefixPlan::freeze_with_instructions(spec(), vec![]).unwrap();
    }
    let elapsed = start.elapsed();
    println!("freeze runs={} total={:?} per-run={:?}", RUNS, elapsed, elapsed / RUNS as u32);
}

#[test]
#[ignore = "perf measurement, not a correctness gate"]
fn which_accessor_costs_what() {
    const RUNS: usize = 5000;
    let plan = PrefixPlan::freeze_with_instructions(spec(), vec![]).unwrap();

    let ops: Vec<(&str, fn(&PrefixPlan) -> usize)> = vec![
        ("common_tools", |p| p.common_tools().unwrap().len()),
        ("common_messages", |p| p.common_messages().unwrap().len()),
        ("common_prefix_bytes", |p| p.common_prefix_bytes().unwrap().len()),
        ("digest_only", |p| p.digest().len()),
    ];
    for (name, f) in ops {
        let start = Instant::now();
        let mut sink = 0usize;
        for _ in 0..RUNS {
            sink += f(&plan);
        }
        let e = start.elapsed();
        println!("{:<22} per-run={:?} sink={}", name, e / RUNS as u32, sink);
    }
    // Registry construction alone, no plan involved.
    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..RUNS {
        sink += sic_core_rust::prefix::common_tool_registry().len();
    }
    let e = start.elapsed();
    println!("{:<22} per-run={:?} sink={}", "common_tool_registry", e / RUNS as u32, sink);
}

#[test]
#[ignore = "perf measurement, not a correctness gate"]
fn validate_is_the_dominant_cost_not_the_clone() {
    const RUNS: usize = 5000;
    let plan = PrefixPlan::freeze_with_instructions(spec(), vec![]).unwrap();
    let data_value = serde_json::to_value(&plan).unwrap();

    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..RUNS {
        sink += sic_core_rust::prefix::common_tool_registry().len();
    }
    let registry = start.elapsed();

    let start = Instant::now();
    for _ in 0..RUNS {
        sink += serde_json::to_value(&plan).unwrap().to_string().len();
    }
    let reserialize = start.elapsed();

    let start = Instant::now();
    for _ in 0..RUNS {
        sink += plan.common_tools().unwrap().len();
    }
    let accessor = start.elapsed();

    println!("registry_only      per-run={:?}", registry / RUNS as u32);
    println!("reserialize_plan   per-run={:?}", reserialize / RUNS as u32);
    println!("common_tools       per-run={:?}", accessor / RUNS as u32);
    println!("  => validate share ~ {:?}", (accessor.saturating_sub(registry)) / RUNS as u32);
    assert!(sink > 0);
    let _ = data_value;
}
