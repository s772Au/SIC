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
async fn chain(core: &Core) -> String {
    call(core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn program_publish(core: &Core, ctx: &OperationContext, source: &str, key: &str) -> String {
    let prepared = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":prepared["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn private_context_has_one_owner_and_never_becomes_a_shared_view() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let id = chain(&core).await;
    let ctx = context("owner", Some(&id));

    let created = call(
        &core,
        &ctx,
        "hierarchy.context.create",
        json!({"context_id":"editor:e1","editor":"e1"}),
    )
    .await;
    assert_eq!(created["owner_editor"], "e1");
    let duplicate = core
        .operate(
            "hierarchy.context.create",
            &ctx,
            &json!({"context_id":"editor:e1","editor":"e1"}),
        )
        .await
        .unwrap_err();
    assert!(
        duplicate.message.contains("already exists"),
        "{duplicate:?}"
    );
    let mismatch = core
        .operate(
            "hierarchy.context.create",
            &ctx,
            &json!({"context_id":"private","editor":"e1"}),
        )
        .await
        .unwrap_err();
    assert!(
        mismatch.message.contains("must be named editor:"),
        "{mismatch:?}"
    );
    let unnamed = core
        .operate(
            "hierarchy.context.create",
            &ctx,
            &json!({"context_id":"editor:blank","editor":""}),
        )
        .await
        .unwrap_err();
    assert!(unnamed.message.contains("bounded nonempty"), "{unnamed:?}");

    // Private material goes through the same WB/CB dual chain, at its own
    // levels, without touching main.
    let private_source = call(
        &core,
        &ctx,
        "hierarchy.context.ingest",
        json!({"context_id":"editor:e1",
               "request":{"messages":[{"role":"user","content":"private note one"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "wb.form",
        json!({"context_id":"editor:e1","source_ids":[private_source]}),
    )
    .await;
    let first = program_publish(&core, &ctx, &private_source, "private-l1").await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"context_id":"editor:e1","level":2,"cb_ids":[first]}),
    )
    .await;
    let second = program_publish(
        &core,
        &ctx,
        higher["source_id"].as_str().unwrap(),
        "private-l2",
    )
    .await;
    assert_ne!(first, second);

    let selected = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"context_id":"editor:e1","level":2,"roles":["editor:e1"]}),
    )
    .await;
    assert_eq!(selected["role_views"]["editor:e1"]["level"], 2);
    assert_eq!(selected["selected_cb_ids"]["editor:e1"], json!([second]));
    let state = call(&core, &ctx, "hierarchy.get", json!({})).await;
    assert_eq!(state["contexts"]["editor:e1"]["owner_editor"], "e1");
    assert_eq!(state["role_views"]["worker"]["context_id"], "main");
    assert_eq!(state["role_views"]["worker"]["level"], 1);
    assert_eq!(state["role_views"]["editor"]["context_id"], "main");

    for (roles, expected) in [
        (json!(["worker"]), "cannot become a shared role view"),
        (json!(["editor"]), "cannot become a shared role view"),
        (json!(["editor:e2"]), "must match the private context owner"),
    ] {
        let error = core
            .operate(
                "hierarchy.select",
                &ctx,
                &json!({"context_id":"editor:e1","level":2,"roles":roles}),
            )
            .await
            .unwrap_err();
        assert!(error.message.contains(expected), "roles {roles}: {error:?}");
    }
    let shared_private_role = core
        .operate(
            "hierarchy.select",
            &ctx,
            &json!({"context_id":"main","level":1,"roles":["editor:e1"]}),
        )
        .await
        .unwrap_err();
    assert!(
        shared_private_role
            .message
            .contains("require an Editor private context"),
        "{shared_private_role:?}"
    );
    let empty_level = core
        .operate(
            "hierarchy.select",
            &ctx,
            &json!({"context_id":"editor:e1","level":3,"roles":["editor:e1"]}),
        )
        .await
        .unwrap_err();
    assert!(
        empty_level.message.contains("no CB")
            || empty_level.message.contains("context or level not found"),
        "{empty_level:?}"
    );
    store.close().await;
}
