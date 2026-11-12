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
        allow_revoke: true,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn publish(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> (String, String) {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    let cb = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source, cb)
}

#[tokio::test]
async fn revocation_blocks_two_level_backtracking_but_keeps_status_redacted() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let (first_source, first_cb) = publish(&core, &ctx, "first fact", "level-one-a").await;
    let (_, second_cb) = publish(&core, &ctx, "second fact", "level-one-b").await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":2,"cb_ids":[first_cb,second_cb]}),
    )
    .await;
    let job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":higher["source_id"],"rules":[]}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let higher_cb = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"level-two"}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let checkpoint = call(
        &core,
        &ctx,
        "chain.checkpoint",
        json!({"cut_id":state["cut_seq"]}),
    )
    .await;

    call(
        &core,
        &ctx,
        "security.classify",
        json!({"source_id":first_source,"policy":"access_revoke"}),
    )
    .await;
    let revoked = call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":first_source,"reason":"multi-level test"}),
    )
    .await;
    assert_eq!(revoked["policy"], "access_revoke");
    assert_eq!(revoked["physical_erasure"], false);

    for payload in [json!({"cb_id":higher_cb}), json!({"cb_id":first_cb})] {
        assert_eq!(
            core.operate("hierarchy.expand", &ctx, &payload)
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
    }
    assert_eq!(
        core.operate("hierarchy.timeline", &ctx, &json!({"level":2}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        core.operate("memory.replay", &ctx, &json!({"cb_id":higher_cb}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    let status = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(status["chain_id"], chain);
    assert_eq!(status["cut_seq"], state["cut_seq"].as_u64().unwrap() + 1);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("first fact")
    );
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(pack["privacy"]["revoked"], true);
    assert!(pack["blocks"].as_array().unwrap().is_empty());
    assert_eq!(
        pack["accounting"]["measurements"]["suppressed"],
        "revocation"
    );
    let _ = checkpoint;
    store.close().await;
}

#[tokio::test]
async fn revocation_blocks_three_level_backtracking() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let mut level_one = Vec::new();
    let mut first_source = String::new();
    for n in 0..4 {
        let (source, cb) = publish(&core, &ctx, &format!("fact {n}"), &format!("deep-{n}")).await;
        if n == 0 {
            first_source = source;
        }
        level_one.push(cb);
    }
    let mut level_two = Vec::new();
    for pair in level_one.chunks(2) {
        let higher = call(
            &core,
            &ctx,
            "hierarchy.form",
            json!({"level":2,"cb_ids":pair}),
        )
        .await;
        let job = call(
            &core,
            &ctx,
            "edit.program",
            json!({"source_id":higher["source_id"],"rules":[]}),
        )
        .await;
        let state = call(&core, &ctx, "chain.get", json!({})).await;
        let cb = call(
            &core,
            &ctx,
            "cut.publish",
            json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":format!("deep-l2-{}", level_two.len())}),
        )
        .await["cb_id"]
            .as_str()
            .unwrap()
            .to_owned();
        level_two.push(cb);
    }
    let highest = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":3,"cb_ids":level_two}),
    )
    .await;
    let job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":highest["source_id"],"rules":[]}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let level_three = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":"deep-l3"}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let checkpoint = call(
        &core,
        &ctx,
        "chain.checkpoint",
        json!({"cut_id":state["cut_seq"]}),
    )
    .await;

    let socket = call(
        &core,
        &ctx,
        "socket.create",
        json!({"checkpoint_id":checkpoint["checkpoint_id"],"acl":{
            "message":[],"signal":[],"spawn":[],"read":[ctx.actor.clone()]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = call(
        &core,
        &ctx,
        "socket.read",
        json!({"socket_id":socket,"offset":0,"limit":100}),
    )
    .await;
    assert!(page["value"].as_str().unwrap().contains("fact 0"));

    call(
        &core,
        &ctx,
        "security.classify",
        json!({"source_id":first_source,"policy":"access_revoke"}),
    )
    .await;
    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":first_source,"reason":"three-level test"}),
    )
    .await;

    let mut targets = vec![
        level_three.clone(),
        level_two[0].clone(),
        level_two[1].clone(),
    ];
    targets.extend(level_one.clone());
    for target in targets {
        assert_eq!(
            core.operate("hierarchy.expand", &ctx, &json!({"cb_id":target}))
                .await
                .unwrap_err()
                .code,
            "forbidden",
            "{target}"
        );
    }
    for level in [2, 3] {
        assert_eq!(
            core.operate("hierarchy.timeline", &ctx, &json!({"level":level}))
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
    }
    assert_eq!(
        core.operate("memory.replay", &ctx, &json!({"cb_id":level_three}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    let status = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(status["cut_seq"], state["cut_seq"].as_u64().unwrap() + 1);
    assert!(!serde_json::to_string(&status).unwrap().contains("fact 0"));
    let checkpoint_read = core
        .operate(
            "socket.read",
            &ctx,
            &json!({"socket_id":socket,"offset":0,"limit":100}),
        )
        .await
        .unwrap_err();
    assert_eq!(checkpoint_read.code, "forbidden");
    assert!(!checkpoint_read.message.contains("fact 0"));
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(pack["privacy"]["revoked"], true);
    assert!(pack["blocks"].as_array().unwrap().is_empty());
    store.close().await;
}
