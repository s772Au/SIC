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
fn policy(max_level: u32, flush: bool, protected: usize) -> Value {
    let mut levels = serde_json::Map::new();
    for level in 2..=max_level {
        levels.insert(level.to_string(), json!(2));
    }
    json!({
        "context_id": "main",
        "min_sources_per_wb": 1,
        "max_sources_per_wb": 1,
        "max_units_per_wb": 64,
        "level_batch_sizes": levels,
        "max_level": max_level,
        "protected_prefix_blocks": protected,
        "flush": flush,
        "max_actions": 256,
        "max_model_calls": 32,
        "editor_mode": "program_fixture",
        "program_rules": ["deduplicate_exact"]
    })
}
async fn published_count(core: &Core, ctx: &OperationContext, level: u32) -> usize {
    call(core, ctx, "chain.get", json!({})).await["contexts"]["main"]["levels"][level.to_string()]
        ["cbs"]
        .as_array()
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn first_order_waits_inside_the_protected_prefix_then_catches_up_at_once() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));

    // Window of 2: one and two pending blocks are held, nothing is published.
    let _a = ingest(&core, &ctx, "block a").await;
    let held_one = call(&core, &ctx, "runtime.schedule", policy(1, false, 2)).await;
    assert!(
        held_one["published"].as_array().unwrap().is_empty(),
        "{held_one}"
    );
    assert!(
        held_one["completed"].as_bool().unwrap_or(false) == false
            || held_one["wait_reason"].is_string()
    );
    assert_eq!(held_one["wait"]["code"], "protected_prefix", "{held_one}");
    assert_eq!(held_one["wait"]["level"], 1);
    assert_eq!(held_one["wait"]["available"], 1);
    assert_eq!(held_one["wait"]["required"], 3);
    assert_eq!(published_count(&core, &ctx, 1).await, 0);

    let _b = ingest(&core, &ctx, "block b").await;
    let held_two = call(&core, &ctx, "runtime.schedule", policy(1, false, 2)).await;
    assert!(
        held_two["published"].as_array().unwrap().is_empty(),
        "{held_two}"
    );
    assert_eq!(held_two["wait"]["available"], 2);
    assert_eq!(published_count(&core, &ctx, 1).await, 0);

    // The third pending block exceeds the window: one CB replaces all three.
    let _c = ingest(&core, &ctx, "block c").await;
    let caught_up = call(&core, &ctx, "runtime.schedule", policy(1, false, 2)).await;
    let published = caught_up["published"].as_array().unwrap();
    assert_eq!(published.len(), 1, "{caught_up}");
    assert_eq!(published[0]["level"], 1);
    let sources = published[0]["source_ids"].as_array().unwrap();
    assert_eq!(sources.len(), 3, "{published:?}");
    assert_eq!(published_count(&core, &ctx, 1).await, 1);

    // After the window the scheduler stops waiting and batches again.
    let _d = ingest(&core, &ctx, "block d").await;
    let normal = call(&core, &ctx, "runtime.schedule", policy(1, false, 2)).await;
    assert_eq!(normal["published"].as_array().unwrap().len(), 1, "{normal}");
    assert!(
        normal["wait_reason"].is_null(),
        "an open level must not wait: {normal}"
    );
}

#[tokio::test]
async fn flush_bypasses_the_protected_prefix_immediately() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let _a = ingest(&core, &ctx, "block a").await;

    let flushed = call(&core, &ctx, "runtime.schedule", policy(1, true, 2)).await;
    let published = flushed["published"].as_array().unwrap();
    assert_eq!(published.len(), 1, "{flushed}");
    assert_eq!(published_count(&core, &ctx, 1).await, 1);
}

#[tokio::test]
async fn zero_window_keeps_the_previous_eager_behaviour() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let _a = ingest(&core, &ctx, "block a").await;

    let eager = call(&core, &ctx, "runtime.schedule", policy(1, false, 0)).await;
    assert_eq!(eager["published"].as_array().unwrap().len(), 1, "{eager}");
    assert!(eager["wait_reason"].is_null(), "{eager}");
}

#[tokio::test]
async fn each_higher_level_holds_its_own_window_then_swallows_the_whole_interval() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));

    // Three first-order blocks so the level-1 catch-up fires and level 2 opens.
    for text in ["block a", "block b", "block c"] {
        let _ = ingest(&core, &ctx, text).await;
    }
    let first = call(&core, &ctx, "runtime.schedule", policy(2, false, 2)).await;
    assert_eq!(first["published"].as_array().unwrap().len(), 1, "{first}");
    assert_eq!(published_count(&core, &ctx, 1).await, 1);

    // Level 2 has one unconsumed lower CB: inside its own window it waits.
    let level_two_held = call(&core, &ctx, "runtime.schedule", policy(2, false, 2)).await;
    assert!(
        level_two_held["published"].as_array().unwrap().is_empty(),
        "{level_two_held}"
    );
    assert_eq!(
        level_two_held["wait"]["code"], "protected_prefix",
        "{level_two_held}"
    );
    assert_eq!(level_two_held["wait"]["level"], 2, "{level_two_held}");
    assert_eq!(published_count(&core, &ctx, 2).await, 0);

    // Level 1's window is already open, so each new block becomes its own CB;
    // level 2 stays inside its window while it holds at most two lower CBs.
    let _d = ingest(&core, &ctx, "block d").await;
    let step_d = call(&core, &ctx, "runtime.schedule", policy(2, false, 2)).await;
    assert_eq!(step_d["published"].as_array().unwrap().len(), 1, "{step_d}");
    assert_eq!(step_d["published"][0]["level"], 1, "{step_d}");
    assert_eq!(step_d["wait"]["code"], "protected_prefix", "{step_d}");
    assert_eq!(step_d["wait"]["available"], 2, "{step_d}");
    assert_eq!(published_count(&core, &ctx, 2).await, 0);

    // The third lower CB exceeds level 2's window: its first replacement covers
    // all three at once, in the same pass that publishes the third lower CB.
    let _e = ingest(&core, &ctx, "block e").await;
    let catch_up = call(&core, &ctx, "runtime.schedule", policy(2, false, 2)).await;
    let published = catch_up["published"].as_array().unwrap();
    assert_eq!(published.len(), 2, "{catch_up}");
    assert_eq!(published[0]["level"], 1, "{catch_up}");
    assert_eq!(published[1]["level"], 2, "{catch_up}");
    assert!(catch_up["wait_reason"].is_null(), "{catch_up}");
    assert_eq!(published_count(&core, &ctx, 1).await, 3);
    assert_eq!(published_count(&core, &ctx, 2).await, 1);

    let higher = call(
        &core,
        &ctx,
        "hierarchy.timeline",
        json!({"context_id":"main","level":2,"kind":"wb","limit":10,"offset":0}),
    )
    .await;
    let rows = higher["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{higher}");
    let members = rows[0]["members"].as_array().unwrap();
    assert_eq!(
        members.len(),
        3,
        "first higher WB covers all three lower CBs: {higher}"
    );
}
