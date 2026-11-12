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
async fn setup(core: &Core) -> (String, Vec<String>, String) {
    let chain = call(core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let source = call(
        core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"user","content":"alpha fact"},
            {"role":"user","content":"beta fact"},
            {"role":"user","content":"gamma fact"}
        ]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let source_object =
        call(core, &ctx, "source.read", json!({"source_id":source})).await["source"].clone();
    let units: Vec<String> = source_object["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|unit| unit["unit_id"].as_str().unwrap().to_owned())
        .collect();
    (chain, units, source)
}

#[tokio::test]
async fn two_editors_merge_disjoint_scopes_and_keep_unassigned_units() {
    let core = Core::new(Store::memory().await.unwrap());
    let (chain, units, source) = setup(&core).await;
    let ctx = context("owner", Some(&chain));
    assert_eq!(units.len(), 3, "fixture expects three units: {units:?}");
    let prepared = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[
            {"editor":"e1","unit_ids":[units[0]]},
            {"editor":"e2","unit_ids":[units[1]]}
        ]}),
    )
    .await;
    let job_id = prepared["job_id"].as_str().unwrap().to_owned();
    assert_eq!(prepared["job"]["assignments"].as_array().unwrap().len(), 2);
    assert_eq!(
        prepared["editable_units"].as_array().unwrap().len(),
        3,
        "the third unit stays unassigned and must remain KEEP"
    );

    // Cross-scope and duplicate submissions are rejected before they merge.
    let cross = core
        .operate(
            "edit.submit",
            &ctx,
            &json!({"job_id":job_id,"editor":"e1","ops":[
                {"op":"REPLACE","target":units[1],"content":"escape attempt"}
            ],"pvi":{}}),
        )
        .await
        .unwrap_err();
    assert!(
        cross.message.contains("outside assigned editable scope"),
        "{cross:?}"
    );

    let first = call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job_id,"editor":"e1","ops":[
            {"op":"REPLACE","target":units[0],"content":"alpha edited"}
        ],"pvi":{}}),
    )
    .await;
    assert_eq!(first["status"], "open");
    assert_eq!(first["pending_editors"], json!(["e2"]));
    let duplicate = core
        .operate(
            "edit.submit",
            &ctx,
            &json!({"job_id":job_id,"editor":"e1","ops":[],"pvi":{}}),
        )
        .await
        .unwrap_err();
    assert!(
        duplicate.message.contains("already submitted"),
        "{duplicate:?}"
    );

    // A partial job cannot be published.
    let early = core
        .operate(
            "cut.publish",
            &ctx,
            &json!({"job_id":job_id,"expected_cut":0,"idempotency_key":"multi-early"}),
        )
        .await
        .unwrap_err();
    assert!(early.message.contains("not ready"), "{early:?}");

    let second = call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job_id,"editor":"e2","ops":[
            {"op":"FLAG","target":units[1],"content":"beta risk"}
        ],"pvi":{}}),
    )
    .await;
    assert_eq!(second["status"], "ready");
    assert_eq!(second["pending_editors"], json!([]));

    // F04 candidate revision: each Editor can revise its own scope, and the
    // other Editor's response is untouched. Revision is still CAS-guarded.
    let revised = call(
        &core,
        &ctx,
        "edit.candidate.update",
        json!({"job_id":job_id,"expected_revision":0,"editor":"e1","ops":[
            {"op":"REPLACE","target":units[0],"content":"alpha revised"}
        ],"pvi":{}}),
    )
    .await;
    assert_eq!(revised["revision"], 1);
    assert_eq!(revised["editor"], "e1");
    let candidate = call(&core, &ctx, "edit.candidate.get", json!({"job_id":job_id})).await;
    assert_eq!(candidate["revision"], 1);
    assert_eq!(
        candidate["responses"]["e1"]["ops"][0]["content"],
        "alpha revised"
    );
    assert_eq!(
        candidate["responses"]["e2"]["ops"][0]["content"],
        "beta risk"
    );
    let stale = core
        .operate(
            "edit.candidate.update",
            &ctx,
            &json!({"job_id":job_id,"expected_revision":0,"editor":"e1","ops":[],"pvi":{}}),
        )
        .await
        .unwrap_err();
    assert!(stale.message.contains("revision changed"), "{stale:?}");
    let ambiguous = core
        .operate(
            "edit.candidate.update",
            &ctx,
            &json!({"job_id":job_id,"expected_revision":1,"ops":[],"pvi":{}}),
        )
        .await
        .unwrap_err();
    assert!(
        ambiguous.message.contains("editor is required"),
        "{ambiguous:?}"
    );
    let cross = core
        .operate(
            "edit.candidate.update",
            &ctx,
            &json!({"job_id":job_id,"expected_revision":1,"editor":"e1","ops":[
                {"op":"REPLACE","target":units[1],"content":"escape attempt"}
            ],"pvi":{}}),
        )
        .await
        .unwrap_err();
    assert!(
        cross.message.contains("outside assigned editable scope"),
        "{cross:?}"
    );
    let revised_second = call(
        &core,
        &ctx,
        "edit.candidate.update",
        json!({"job_id":job_id,"expected_revision":1,"editor":"e2","ops":[
            {"op":"REPLACE","target":units[1],"content":"beta revised"}
        ],"pvi":{}}),
    )
    .await;
    assert_eq!(revised_second["revision"], 2);

    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job_id,"expected_cut":0,"idempotency_key":"multi-publish"}),
    )
    .await;
    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    let cbs = cut["view"]["cbs"].as_array().unwrap();
    assert_eq!(cbs.len(), 1, "one merged CB must be published");
    let cb = &cbs[0];
    assert_eq!(cb["cb_id"], published["cb_id"]);
    let body = cb["body"].as_str().unwrap();
    assert!(body.contains("alpha revised"), "{body}");
    assert!(body.contains("beta revised"), "{body}");
    assert!(
        body.contains("gamma fact"),
        "unassigned unit must stay KEEP: {body}"
    );
    let editors = cb["editors"].as_array().unwrap();
    assert_eq!(editors.len(), 2);
    assert_eq!(editors[0]["editor"], "e1");
    assert_eq!(editors[1]["editor"], "e2");
}

#[tokio::test]
async fn overlapping_or_incomplete_assignments_are_rejected() {
    let core = Core::new(Store::memory().await.unwrap());
    let (chain, units, source) = setup(&core).await;
    let ctx = context("owner", Some(&chain));
    for assignments in [
        json!([{"editor":"e1","unit_ids":[units[0]]},{"editor":"e2","unit_ids":[units[0]]}]),
        json!([{"editor":"e1","unit_ids":[units[0]]},{"editor":"e1","unit_ids":[units[1]]}]),
        json!([{"editor":"e1","unit_ids":[]}]),
        json!([]),
    ] {
        let error = core
            .operate(
                "edit.prepare",
                &ctx,
                &json!({"source_id":source,"assignments":assignments}),
            )
            .await
            .unwrap_err();
        assert!(
            error.message.contains("assignment") || error.message.contains("Editor"),
            "unexpected error: {error:?}"
        );
    }
}

#[tokio::test]
async fn multi_editor_dispatch_requires_an_explicit_editor() {
    let core = Core::new(Store::memory().await.unwrap());
    let (chain, units, source) = setup(&core).await;
    let mut ctx = context("owner", Some(&chain));
    ctx.allow_model = true;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[
            {"editor":"e1","unit_ids":[units[0]]},
            {"editor":"e2","unit_ids":[units[1]]}
        ]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Without an editor id the Core cannot know which scope to dispatch.
    let ambiguous = core
        .operate("edit.run", &ctx, &json!({"job_id":job}))
        .await
        .unwrap_err();
    assert!(
        ambiguous.message.contains("editor is required"),
        "{ambiguous:?}"
    );
    // With an editor but no frozen model profile the dispatch stops before
    // any provider call.
    let unconfigured = core
        .operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
        .await
        .unwrap_err();
    assert_eq!(unconfigured.code, "model_unconfigured");
}
