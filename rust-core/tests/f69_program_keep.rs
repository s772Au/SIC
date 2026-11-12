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

#[tokio::test]
async fn late_tool_result_is_program_kept_not_claiming_editor_saw_it() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"assistant","content":"working",
             "tool_calls":[{"id":"call-late","type":"function",
                            "function":{"name":"read","arguments":"{}"}}]}
        ]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let prepared = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await;
    let job = prepared["job_id"].as_str().unwrap().to_owned();
    assert_eq!(prepared["editable_units"].as_array().unwrap().len(), 1);
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":"working edited"}],
               "pvi":{}}),
    )
    .await;

    // The tool result arrives after the frozen Editor snapshot and closes the
    // open native call in the same source group.
    let late = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"tool","tool_call_id":"call-late","content":"late result"}
        ]}}),
    )
    .await;
    let late_source = late["source_id"].as_str().unwrap().to_owned();
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":"late-keep"}),
    )
    .await;
    let cb = published["cb_id"].as_str().unwrap().to_owned();
    let body = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"][0].clone();
    assert!(body["body"].as_str().unwrap().contains("working edited"));
    assert!(body["body"].as_str().unwrap().contains("late result"));
    assert_eq!(
        body["program_kept_after_snapshot"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let late_unit = body["program_kept_after_snapshot"][0]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(late_unit.ends_with(":u1"), "{late_unit}");
    assert!(
        !body["editors"][0]["unit_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(late_unit)),
        "the late unit must not appear in the Editor scope"
    );
    let kept = body["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|op| op["origin"] == "post-snapshot-program")
        .expect("explicit late KEEP op");
    assert_eq!(kept["op"], "KEEP");
    assert_eq!(
        kept["reason"],
        "late protocol closure material; not supplied to Editor"
    );
    assert_eq!(body["editor_source_snapshot_ids"], json!([source]));
    assert_eq!(body["editors"][0]["unit_ids"], json!(["u1"]));
    // The CB records the current group (root + late part), while the frozen
    // Editor source list and expandable WB membership stay at the root.
    assert_eq!(body["source_ids"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["source_ids"][1].as_str().unwrap(),
        late_unit.trim_end_matches(":u1")
    );
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":cb})).await;
    assert_eq!(expanded["source_ids"], json!([source]));
    assert!(
        !expanded["original_units"]
            .as_array()
            .unwrap()
            .iter()
            .any(|unit| unit["text"] == "late result"),
        "the late part is not part of the Editor's frozen WB membership"
    );
    let _ = late_source;
}
