use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn context(chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
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

#[tokio::test]
async fn schedule_waits_on_unresolved_native_tool_calls_before_forming_a_wb() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(&chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"assistant","content":"searching",
             "tool_calls":[{"id":"call-open","type":"function","function":{"name":"read","arguments":"{}"}}]}
        ]}}),
    )
    .await;
    assert_eq!(source["open_calls"], json!(["call-open"]));
    let scheduled = call(
        &core,
        &ctx,
        "runtime.schedule",
        json!({"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":1,
               "max_units_per_wb":64,"level_batch_sizes":{},"max_level":1,
               "flush":true,"max_actions":16,"max_model_calls":0,
               "editor_mode":"program_fixture","program_rules":[]}),
    )
    .await;
    assert!(scheduled["published"].as_array().unwrap().is_empty());
    assert_eq!(scheduled["wait"]["code"], "unresolved_tool_calls");
    assert!(
        scheduled["wait_reason"]
            .as_str()
            .unwrap()
            .contains("unresolved native tool calls")
    );
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 0);
    assert!(
        state["contexts"]["main"]["levels"]["1"]["wbs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
