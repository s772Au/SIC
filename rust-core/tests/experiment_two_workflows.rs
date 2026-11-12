use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    experiment::{
        EvaluationStatus, Rubric, RubricDimension, exact_fact_retention, prefix_baseline,
        score_rubric,
    },
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
        "context_id": "main",
        "min_sources_per_wb": 1,
        "max_sources_per_wb": 2,
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
async fn two_workflows_are_executable_and_scored_by_exact_fact_retention() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let mut facts = Vec::new();
    let mut texts = Vec::new();
    for task in 1..=2 {
        for index in 1..=5 {
            let fact = format!("T{task}F{index}");
            let text = format!("{fact} {} end", "context ".repeat(20));
            facts.push(fact);
            texts.push(text.clone());
            call(
                &core,
                &ctx,
                "source.ingest",
                json!({"request":{"messages":[{"role":"user","content":text}]}}),
            )
            .await;
        }
    }

    // Workflow A: SIC program scheduling over the same task material.
    let report = call(&core, &ctx, "runtime.schedule", policy()).await;
    assert_eq!(report["published"].as_array().unwrap().len(), 5);
    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    let blocks = cut["view"]["cbs"].as_array().unwrap();
    assert_eq!(blocks.len(), 5);
    let sic_material = blocks
        .iter()
        .filter_map(|block| block["body"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let sic = exact_fact_retention(&facts, &sic_material);
    assert_eq!(sic.expected, 10);
    assert_eq!(sic.retained, 10);
    assert_eq!(sic.ratio, 1.0);

    // Workflow B: fixed-budget prefix baseline over the same source order.
    let total_chars = texts.iter().map(|text| text.chars().count()).sum::<usize>();
    let baseline_material = prefix_baseline(&texts, total_chars / 2);
    let baseline = exact_fact_retention(&facts, &baseline_material);
    assert!(baseline.ratio < 1.0, "{baseline:?}");
    assert!(baseline.ratio < sic.ratio);

    // The pack keeps provenance without copying task material.
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    let pack_text = serde_json::to_string(&pack).unwrap();
    for fact in &facts {
        assert!(!pack_text.contains(fact), "pack leaked {fact}");
    }
    assert_eq!(pack["privacy"]["source_text_included"], false);
    assert_eq!(pack["sources"].as_array().unwrap().len(), 10);

    // The synthetic arm always passes through the rubric gate as not
    // evaluated: a retention score alone is not task quality.
    let rubric = Rubric {
        version: "synthetic-r1".into(),
        reference: "synthetic facts only".into(),
        dimensions: vec![RubricDimension {
            name: "fact_recall".into(),
            weight: 1.0,
        }],
    };
    let mut evidence = std::collections::BTreeMap::new();
    evidence.insert("fact_recall".to_string(), Some(sic.ratio));
    let gated = score_rubric(&rubric, &evidence, "fixture");
    assert_eq!(gated.status, EvaluationStatus::NotEvaluated);
    assert_eq!(gated.score, None);
}
