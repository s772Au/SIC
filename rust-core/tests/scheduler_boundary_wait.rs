//! F30 / F32 / F33 scheduling-space coverage.
//!
//! These tests exercise the *scheduler's decision*, not model quality. A
//! declared wait is a host prediction; these cases pin down that the scheduler
//! keeps declarations and real tool intervals apart, that it records which
//! advance it chose, and that an overtaking policy still cannot run without a
//! host budget.

use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    scheduler::{SchedulePolicy, SchedulerPolicy},
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

async fn try_call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> String {
    match core.operate(op, ctx, &payload).await {
        Ok(_) => "ok".into(),
        Err(error) => error.code.into(),
    }
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

fn policy(min_sources: usize, flush: bool) -> Value {
    json!({
        "context_id": "main",
        "min_sources_per_wb": min_sources,
        "max_sources_per_wb": 4,
        "max_units_per_wb": 512,
        "level_batch_sizes": {},
        "max_level": 1,
        "flush": flush,
        "max_actions": 128,
        "max_model_calls": 32,
        "editor_mode": "program_fixture",
        "program_rules": ["deduplicate_exact"]
    })
}

async fn setup(_min_sources: usize, _flush: bool) -> (Core, OperationContext) {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    ingest(&core, &ctx, "first material").await;
    (core, ctx)
}

#[tokio::test]
async fn declared_wait_is_stored_apart_from_tool_intervals() {
    let (core, ctx) = setup(1, true).await;

    let declared = call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w1","seconds":30}),
    )
    .await;
    assert_eq!(declared["wait"]["status"], "declared");
    // A declaration predicts work; it must never claim a tool started.
    assert_eq!(declared["wait"]["predicts_tool_start"], false);

    let snapshot = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(snapshot["declared_waits"]["w1"]["seconds"], 30);
    // No host tool task exists, so no measured tool interval was invented.
    assert_eq!(call(&core, &ctx, "tools.tasks", json!({})).await["tasks"], json!([]));

    let accounting = call(&core, &ctx, "accounting.get", json!({})).await;
    let tool_intervals = accounting["resources"]["timing"]["intervals"]
        .as_array()
        .map(|rows| rows.iter().filter(|row| row["role"] == "tool").count())
        .unwrap_or(0);
    assert_eq!(tool_intervals, 0, "a declared wait is not a tool interval");
}

#[tokio::test]
async fn closing_a_wait_removes_it_from_the_prediction_set() {
    let (core, ctx) = setup(1, true).await;
    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w1","seconds":30}),
    )
    .await;
    let report = call(&core, &ctx, "runtime.schedule", policy(1, true)).await;
    assert_eq!(report["declared_waits"][0]["wait_id"], "w1");

    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"complete","wait_id":"w1"}),
    )
    .await;
    let after = call(&core, &ctx, "runtime.schedule", policy(1, true)).await;
    assert!(
        after["declared_waits"].as_array().unwrap().is_empty(),
        "a completed declaration no longer predicts anything"
    );
}

#[tokio::test]
async fn estimate_absent_is_reported_null_rather_than_invented() {
    let (core, ctx) = setup(1, true).await;
    let report = call(&core, &ctx, "runtime.schedule", policy(1, true)).await;
    assert!(report["editor_estimate_ms"].is_null());
    assert!(report["editor_estimate_source"].is_null());
}

#[tokio::test]
async fn declared_wait_covering_the_estimate_forms_a_block_without_a_permit() {
    let (core, ctx) = setup(2, false).await;
    // Host declares both the estimate and a wait window it predicts will hold.
    call(&core, &ctx, "config.update", json!({"editor_estimate_ms": 5000})).await;
    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w1","seconds":30}),
    )
    .await;

    let report = call(&core, &ctx, "runtime.schedule", policy(2, false)).await;
    assert_eq!(report["decision"], "overlap-declared-wait");
    assert_eq!(report["editor_estimate_ms"], 5000);
    assert_eq!(report["editor_estimate_source"], "config.editor_estimate_ms");
    // Adopting in time is not an overtaking advance, so no permit was spent.
    assert!(report["interleaves"].as_array().unwrap().is_empty());
    assert_eq!(report["published"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn short_declared_wait_does_not_unlock_an_early_block() {
    let (core, ctx) = setup(2, false).await;
    call(&core, &ctx, "config.update", json!({"editor_estimate_ms": 30000})).await;
    call(
        &core,
        &ctx,
        "runtime.wait",
        json!({"action":"declare","wait_id":"w1","seconds":5}),
    )
    .await;

    let report = call(&core, &ctx, "runtime.schedule", policy(2, false)).await;
    assert!(
        report["published"].as_array().unwrap().is_empty(),
        "a wait shorter than the estimate must not unlock the block"
    );
    assert_eq!(report["needs_flush"], true);
    assert_eq!(report["decision"], "wait-editor");
}

#[tokio::test]
async fn overtaking_policy_without_a_budget_is_refused_by_config() {
    let (core, ctx) = setup(1, true).await;
    // "advance" must be paid for; a zero budget cannot select it.
    let refused = try_call(
        &core,
        &ctx,
        "config.update",
        json!({"schedule_policy":"advance"}),
    )
    .await;
    assert_eq!(refused, "invalid");

    // With a budget the same policy is accepted.
    call(
        &core,
        &ctx,
        "config.update",
        json!({"schedule_policy":"advance","interleave_budget":2}),
    )
    .await;
    let config = call(&core, &ctx, "config.get", json!({})).await;
    assert_eq!(config["configuration"]["schedule_policy"], "advance");
    assert_eq!(config["configuration"]["interleave_budget"], 2);

    // wait remains accepted and needs no budget.
    let back = try_call(&core, &ctx, "config.update", json!({"schedule_policy":"wait"}))
        .await;
    assert_eq!(back, "ok");
}

#[tokio::test]
async fn unknown_schedule_policy_is_rejected() {
    let (core, ctx) = setup(1, true).await;
    let rejected = try_call(
        &core,
        &ctx,
        "config.update",
        json!({"schedule_policy":"interleave","interleave_budget":3}),
    )
    .await;
    assert_eq!(rejected, "invalid");
}

#[tokio::test]
async fn per_wb_budget_charges_once_and_refuses_a_second_permit() {
    let (core, ctx) = setup(1, true).await;
    let wb = call(&core, &ctx, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]
        ["pending"][0]
        .as_str()
        .unwrap()
        .to_owned();

    // The host sets the budget for one WB, then spends it in two steps.
    let set = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":wb,"budget":2}),
    )
    .await;
    assert_eq!(set["budget"], 2);
    assert_eq!(set["remaining"], 2);

    let first = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":wb,"permit_id":"p1","cost":2}),
    )
    .await;
    assert_eq!(first["consumed"], true);
    assert_eq!(first["remaining"], 0);

    // Re-sending the same permit replays it and does not charge twice.
    let replay = call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":wb,"permit_id":"p1","cost":2}),
    )
    .await;
    assert_eq!(replay["consumed"], false);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["remaining"], 0);

    // A different permit on the same WB is refused: the budget is per WB.
    let exhausted = try_call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":wb,"permit_id":"p2","cost":1}),
    )
    .await;
    assert_eq!(exhausted, "budget_exhausted");
}

#[tokio::test]
async fn missing_budget_is_recorded_as_a_decision_not_a_silent_continue() {
    let (core, ctx) = setup(2, false).await;
    // No chain budget and no per-WB budget: the early block cannot be paid for.
    let report = call(&core, &ctx, "runtime.schedule", policy(2, false)).await;
    assert_eq!(report["decision"], "wait-editor");
    assert_eq!(report["needs_flush"], true);
    assert_eq!(report["published"].as_array().unwrap().len(), 0);
    assert_eq!(report["wait"]["code"], "interleave_budget_missing");
}

#[tokio::test]
async fn advance_policy_names_the_exhaustion_reason() {
    let (core, ctx) = setup(2, false).await;
    // An overtaking policy that cannot pay reports the budget reason instead
    // of pretending the pass simply waited.
    call(
        &core,
        &ctx,
        "config.update",
        json!({"schedule_policy":"advance","interleave_budget":1}),
    )
    .await;
    // Spend the WB budget on the one pending block, then add fresh material so
    // the scheduler must ask for another permit it cannot get.
    let wb = call(&core, &ctx, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]
        ["pending"][0]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "runtime.budget",
        json!({"wb_source_id":wb,"budget":1}),
    )
    .await;
    let first = call(&core, &ctx, "runtime.schedule", policy(2, false)).await;
    assert_eq!(first["interleaves"].as_array().unwrap().len(), 1);
    assert_eq!(first["interleaves"][0]["cost"], 1);

    let second = call(&core, &ctx, "runtime.schedule", policy(2, false)).await;
    assert!(
        second["published"].as_array().unwrap().is_empty(),
        "a spent WB budget must not release another block"
    );
}

#[test]
fn schedule_policy_wire_names_are_stable() {
    // The config surface and the Rust enum must not drift apart.
    for (wire, expected) in [
        ("wait", SchedulePolicy::Wait),
        ("advance", SchedulePolicy::Advance),
        ("adaptive", SchedulePolicy::Adaptive),
    ] {
        let decoded: SchedulePolicy = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(decoded, expected);
    }
    // The default stays conservative.
    assert_eq!(SchedulePolicy::default(), SchedulePolicy::Wait);
    assert_eq!(SchedulerPolicy::default().max_level, 1);
}
