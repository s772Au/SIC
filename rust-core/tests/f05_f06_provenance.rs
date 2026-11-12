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
    core.operate(op, ctx, &payload).await.unwrap()["result"].clone()
}
async fn ingest(core: &Core, ctx: &OperationContext, messages: Value) -> String {
    call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":messages}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn publish(core: &Core, ctx: &OperationContext, job_id: &str, key: &str) -> String {
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job_id,"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn protocol_sidecar_survives_three_fragment_wb_and_unit_edit() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));

    let first = ingest(
        &core,
        &ctx,
        json!([{"role":"user","content":"fragment one: {\"k\":1}"}]),
    )
    .await;
    let second = ingest(
        &core,
        &ctx,
        json!([
            {"role":"user","content":"fragment two"},
            {"role":"assistant","content":null,
             "tool_calls":[{"id":"call-2","type":"function",
                            "function":{"name":"read","arguments":"{\"path\":\"x\"}"}}]},
            {"role":"tool","tool_call_id":"call-2","content":"tool result two"}
        ]),
    )
    .await;
    let third = ingest(
        &core,
        &ctx,
        json!([{"role":"user","content":"fragment three"}]),
    )
    .await;

    // F05: the accepted native request is the immutable sidecar; only text
    // fields become editable units and controls stay in the sidecar.
    let sidecar =
        call(&core, &ctx, "source.read", json!({"source_id":second})).await["source"].clone();
    assert_eq!(sidecar["sidecar"]["messages"][2]["tool_call_id"], "call-2");
    assert_eq!(
        sidecar["sidecar"]["messages"][1]["tool_calls"][0]["function"]["name"],
        "read"
    );
    let unit_paths: Vec<Value> = sidecar["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["anchor"]["native_path"].clone())
        .collect();
    assert!(unit_paths.contains(&json!(["messages", 0, "content"])));
    assert!(unit_paths.contains(&json!(["messages", 2, "content"])));
    assert!(
        !unit_paths.contains(&json!(["messages", 2, "role"])),
        "roles are protocol, not editable text"
    );

    // F06: three fragments form one ordered WB; their source identity stays
    // visible in the frozen snapshot.
    call(
        &core,
        &ctx,
        "wb.form",
        json!({"source_ids":[first, second, third]}),
    )
    .await;
    let prepared = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":first,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await;
    assert!(
        prepared["job"]["source_snapshot"]["source_ids"]
            .as_array()
            .unwrap()
            .len()
            == 3
    );
    let units = prepared["editable_units"].as_array().unwrap().clone();
    assert_eq!(units.len(), 4, "3 user texts + one tool result");
    let target = units
        .iter()
        .find(|unit| unit["text"] == "fragment two")
        .unwrap()["unit_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let anchors: Vec<Value> = units
        .iter()
        .map(|unit| unit["anchor"]["source_id"].clone())
        .collect();
    assert!(anchors.contains(&json!(first)));
    assert!(anchors.contains(&json!(second)));
    assert!(anchors.contains(&json!(third)));
    let job_id = prepared["job_id"].as_str().unwrap().to_owned();
    // The assignment must be the frozen scope; adjust it to every unit.
    let all_units: Vec<Value> = units.iter().map(|unit| unit["unit_id"].clone()).collect();
    let assignment_job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":first,"assignments":[{"editor":"e1","unit_ids":all_units}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(job_id, assignment_job);
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":assignment_job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":target,"content":"fragment two edited"}],
               "pvi":{}}),
    )
    .await;
    let cb = publish(&core, &ctx, &assignment_job, "f05-f06").await;
    let body = call(&core, &ctx, "cut.get", json!({})).await["view"]["cbs"][0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(body.contains("fragment two edited"), "{body}");
    assert!(body.contains("fragment one"), "{body}");
    assert!(body.contains("fragment three"), "{body}");
    assert!(body.contains("tool result two"), "{body}");

    // Editing a unit does not rewrite the raw archive: expand returns the
    // original text with its source anchor.
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":cb})).await;
    assert_eq!(expanded["source_ids"], json!([first, second, third]));
    let originals = expanded["original_units"].as_array().unwrap();
    let original = originals
        .iter()
        .find(|unit| unit["text"] == "fragment two")
        .expect("raw archive keeps the pre-edit text");
    assert_eq!(original["anchor"]["source_id"], second);
    assert!(
        originals
            .iter()
            .all(|unit| unit["text"] != "fragment two edited"),
        "expand must not return the edited projection as original material"
    );
    // The sidecar still round-trips the original tool linkage after publish.
    let sidecar_after =
        call(&core, &ctx, "source.read", json!({"source_id":second})).await["source"].clone();
    assert_eq!(
        sidecar_after["sidecar"]["messages"][2]["tool_call_id"],
        "call-2"
    );
    store_close(core).await;
}

async fn store_close(core: Core) {
    core.store.close().await;
}
