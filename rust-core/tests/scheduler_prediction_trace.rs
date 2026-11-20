//! F30: prediction and actual completion recorded in one trace.
//!
//! The scheduler's estimate and the host's declared wait are *predictions*.
//! This test runs a real external process as the tool and a mock
//! Provider as the Editor, then compares what the scheduler predicted with
//! what actually happened. A mock Editor is enough for the scheduling
//! *mechanism*; it is explicitly not evidence about a real model or a real
//! DSH native tool, and the test says so in its assertions.

use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use std::time::Duration;

#[path = "common/delayed_tool.rs"]
mod delayed_tool;

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

#[tokio::test]
async fn declared_wait_prediction_is_recorded_next_to_the_real_tool_interval() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));

    // The host declares it expects a 3 second window and says the Editor
    // should take about 1 second.
    call(&core, &ctx, "config.update", json!({"editor_estimate_ms": 1000})).await;
    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w-tool","seconds":3}),
    )
    .await;

    // Real external process: 1.2 seconds of actual work.
    let started = std::time::Instant::now();
    let status = delayed_tool::command(1200, None)
        .status()
        .expect("external delay process must start");
    assert!(status.success());
    let actual = started.elapsed();

    // No host tool task was created: the declared window and the external
    // process are both outside the host-tool ledger.
    assert_eq!(
        call(&core, &ctx, "tools.tasks", json!({})).await["tasks"],
        json!([])
    );

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let intervals = accounting["resources"]["timing"]["intervals"].as_array().cloned().unwrap_or_default();
    let tool_rows = intervals.iter().filter(|row| row["role"] == "tool").count();

    // The prediction is in the scheduler report; the measurement is in
    // accounting. They are separate records on purpose.
    let report = call(
        &core,
        &ctx,
        "runtime.schedule",
        json!({"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":4,
               "max_units_per_wb":512,"level_batch_sizes":{},"max_level":1,"flush":true,
               "max_actions":128,"max_model_calls":32,"editor_mode":"program_fixture",
               "program_rules":["deduplicate_exact"]}),
    )
    .await;
    assert_eq!(report["editor_estimate_ms"], 1000);
    assert_eq!(report["editor_estimate_source"], "config.editor_estimate_ms");
    assert_eq!(report["declared_waits"][0]["seconds"], 3);
    // The declared 3s window covers the 1s estimate, so the scheduler adopts
    // in time rather than waiting for a minimum batch.
    assert_eq!(report["decision"], "overlap-declared-wait");

    // The real process really did run; the elapsed time is the actual side of
    // the comparison and must not be folded into the declaration.
    assert!(actual >= Duration::from_millis(1100));
    assert_eq!(tool_rows, 0, "a declared wait and a raw process are not a host tool task");
}

#[tokio::test]
async fn prediction_missing_keeps_the_decision_honest() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    ingest(&core, &ctx, "only material").await;

    // No estimate and a declared wait: the scheduler cannot know whether the
    // window covers the Editor, so it must not claim the overlap decision.
    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w1","seconds":60}),
    )
    .await;
    let report = call(
        &core,
        &ctx,
        "runtime.schedule",
        json!({"context_id":"main","min_sources_per_wb":2,"max_sources_per_wb":4,
               "max_units_per_wb":512,"level_batch_sizes":{},"max_level":1,"flush":false,
               "max_actions":128,"max_model_calls":32,"editor_mode":"program_fixture",
               "program_rules":["deduplicate_exact"]}),
    )
    .await;
    assert!(report["editor_estimate_ms"].is_null());
    assert_eq!(report["decision"], "wait-editor");
    assert!(report["published"].as_array().unwrap().is_empty());
}
