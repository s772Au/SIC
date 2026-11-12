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
fn policy() -> Value {
    json!({
        "context_id": "editor:e1",
        "min_sources_per_wb": 1,
        "max_sources_per_wb": 1,
        "max_units_per_wb": 512,
        "level_batch_sizes": {"2": 2, "3": 2},
        "max_level": 3,
        "flush": false,
        "max_actions": 128,
        "max_model_calls": 32,
        "editor_mode": "program_fixture",
        "program_rules": ["deduplicate_exact"]
    })
}

#[tokio::test]
async fn one_schedule_call_forms_private_levels_2_and_3() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    call(
        &core,
        &ctx,
        "hierarchy.context.create",
        json!({"context_id":"editor:e1","editor":"e1"}),
    )
    .await;
    for n in 0..4 {
        call(
            &core,
            &ctx,
            "hierarchy.context.ingest",
            json!({"context_id":"editor:e1",
                   "request":{"messages":[{"role":"user","content":format!("private {n}")}]}}),
        )
        .await;
    }
    let report = call(&core, &ctx, "runtime.schedule", policy()).await;
    let published = report["published"].as_array().unwrap();
    assert_eq!(published.len(), 7, "{report}");
    assert_eq!(published.iter().filter(|row| row["level"] == 1).count(), 4);
    assert_eq!(published.iter().filter(|row| row["level"] == 2).count(), 2);
    assert_eq!(published.iter().filter(|row| row["level"] == 3).count(), 1);
    let hierarchy = call(&core, &ctx, "hierarchy.get", json!({})).await;
    assert_eq!(
        hierarchy["contexts"]["editor:e1"]["levels"]["3"]["cbs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        hierarchy["contexts"]["main"]["levels"]["1"]["cbs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "private recursion must not publish into the shared main context"
    );
    let selected = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"context_id":"editor:e1","roles":["editor:e1"],"level":3}),
    )
    .await;
    assert_eq!(selected["role_views"]["editor:e1"]["level"], 3);
    let cb = hierarchy["contexts"]["editor:e1"]["levels"]["3"]["cbs"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":cb})).await;
    // The level-3 CB expands to its two level-2 members; the private chain
    // itself never enters the shared main context.
    assert_eq!(expanded["context_id"], "editor:e1");
    assert_eq!(expanded["level"], 3);
    assert!(!expanded["source_ids"].as_array().unwrap().is_empty());
    let members = expanded["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    let level2 = hierarchy["contexts"]["editor:e1"]["levels"]["2"]["cbs"]
        .as_array()
        .unwrap();
    for member in members {
        assert_eq!(member["level"], 2);
        let id = member["cb_id"].as_str().unwrap();
        assert!(
            level2.iter().any(|cb| cb == id),
            "member {id} is not a level-2 CB"
        );
    }
    assert_eq!(expanded["original_units"].as_array().unwrap().len(), 2);
}
