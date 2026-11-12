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
async fn core_rejects_image_ingress_until_it_can_reach_the_worker_losslessly() {
    let core = Core::new(Store::memory().await.unwrap());
    let id = chain(&core).await;
    let ctx = context("owner", Some(&id));
    let input = json!({"request":{"messages":[{"role":"user","content":[
        {"type":"text","text":"What is shown?"},
        {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}
    ]}]}});
    assert_eq!(
        core.operate("source.ingest", &ctx, &input)
            .await
            .unwrap_err()
            .code,
        "media_not_implemented"
    );
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["groups"], json!({}));
}

#[tokio::test]
async fn immutable_dual_chain_publication_routes_pvi_and_replay() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let id = chain(&core).await;
    let ctx = context("owner", Some(&id));
    let a = ingest(&core, &ctx, "first 中文").await;
    let b = ingest(&core, &ctx, "second 🙂").await;
    let wb = call(&core, &ctx, "wb.form", json!({"source_ids":[a,b]})).await["wb_id"].clone();
    let prepared=call(&core,&ctx,"edit.program",json!({"source_id":a,"rules":[],"pvi":{"P":["P fact"],"V":["V next"],"I":{"status":"latest"}}})).await;
    let payload =
        json!({"job_id":prepared["job_id"],"expected_cut":0,"idempotency_key":"publish-one"});
    let first = core.operate("cut.publish", &ctx, &payload).await.unwrap();
    let result = &first["result"];
    assert_eq!(result["wb_id"], wb);
    let cb = result["cb_id"].as_str().unwrap();
    let duplicate = core.operate("cut.publish", &ctx, &payload).await.unwrap();
    assert_eq!(duplicate["result"], *result);
    assert_eq!(duplicate["evidence"]["replayed"], true);
    let changed =
        json!({"job_id":prepared["job_id"],"expected_cut":1,"idempotency_key":"publish-one"});
    assert!(core.operate("cut.publish", &ctx, &changed).await.is_err());
    let view = call(&core, &ctx, "cut.get", json!({})).await;
    assert_eq!(
        view["view"]["cbs"][0]["body"],
        "first 中文\nsecond 🙂\nP fact"
    );
    assert_eq!(view["view"]["instant"]["status"], "latest");
    assert_eq!(view["pending"].as_array().unwrap().len(), 1);
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":cb})).await;
    assert_eq!(expanded["wb_id"], wb);
    assert_eq!(expanded["source_ids"], json!([a, b]));
    let replay = call(&core, &ctx, "memory.replay", json!({"cb_id":cb})).await;
    assert_eq!(replay["body"], view["view"]["cbs"][0]["body"]);
    assert_eq!(replay["external_effects_replayed"], false);
    let wb_timeline = call(
        &core,
        &ctx,
        "hierarchy.timeline",
        json!({"level":1,"kind":"wb"}),
    )
    .await;
    let cb_timeline = call(
        &core,
        &ctx,
        "hierarchy.timeline",
        json!({"level":1,"kind":"cb"}),
    )
    .await;
    // V is immediately pending, but its WB forms at the next edit boundary
    // instead of freezing a singleton before later material can join it.
    assert_eq!(wb_timeline["total"], 1);
    assert_eq!(cb_timeline["total"], 1);
    assert_eq!(wb_timeline["rows"][0]["cb_id"], cb);
    let volatile_source = view["pending"][0].as_str().unwrap();
    let next_cb = program_publish(&core, &ctx, volatile_source, "publish-volatile").await;
    let expanded = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":next_cb})).await;
    assert_eq!(expanded["original_units"][0]["text"], "V next");
    assert_eq!(
        call(
            &core,
            &ctx,
            "hierarchy.timeline",
            json!({"level":1,"kind":"wb"})
        )
        .await["total"],
        2
    );
    store.close().await;
}

#[tokio::test]
async fn real_third_order_materialization_and_uncovered_tail_are_reversible() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let id = chain(&core).await;
    let ctx = context("owner", Some(&id));
    let mut ones = Vec::new();
    for n in 0..4 {
        let s = ingest(&core, &ctx, &format!("fact {n}")).await;
        ones.push(program_publish(&core, &ctx, &s, &format!("base-{n}")).await);
    }
    let mut twos = Vec::new();
    for (n, pair) in ones.chunks(2).enumerate() {
        let higher = call(
            &core,
            &ctx,
            "hierarchy.form",
            json!({"level":2,"cb_ids":pair}),
        )
        .await;
        twos.push(
            program_publish(
                &core,
                &ctx,
                higher["source_id"].as_str().unwrap(),
                &format!("two-{n}"),
            )
            .await,
        );
    }
    let formed = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":3,"cb_ids":twos}),
    )
    .await;
    let top = program_publish(&core, &ctx, formed["source_id"].as_str().unwrap(), "third").await;
    let selected = call(
        &core,
        &ctx,
        "hierarchy.select",
        json!({"level":3,"roles":["worker","editor"]}),
    )
    .await;
    assert_eq!(selected["selected_cb_ids"]["worker"], json!([top]));
    let first = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":top})).await;
    assert_eq!(first["members"][0]["cb_id"], twos[0]);
    let second = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":twos[0]})).await;
    assert_eq!(second["members"][0]["cb_id"], ones[0]);
    let base = call(&core, &ctx, "hierarchy.expand", json!({"cb_id":ones[0]})).await;
    assert_eq!(base["original_units"][0]["text"], "fact 0");
    let s = ingest(&core, &ctx, "later tail").await;
    let tail = program_publish(&core, &ctx, &s, "tail").await;
    let view = call(&core, &ctx, "cut.get", json!({})).await;
    assert_eq!(view["view"]["cbs"].as_array().unwrap().len(), 2);
    assert_eq!(view["view"]["cbs"][0]["cb_id"], top);
    assert_eq!(view["view"]["cbs"][1]["cb_id"], tail);
    assert!(
        core.operate(
            "hierarchy.form",
            &ctx,
            &json!({"level":2,"cb_ids":[ones[0],ones[1]]})
        )
        .await
        .is_err()
    );
    store.close().await;
}

#[tokio::test]
async fn auth_and_revocation_do_not_leak_or_rewrite_source() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let id = chain(&core).await;
    let mut owner = context("owner", Some(&id));
    let source = ingest(&core, &owner, "sensitive fixture").await;
    let cb = program_publish(&core, &owner, &source, "p").await;
    let stranger = context("stranger", Some(&id));
    assert!(
        core.operate("chain.get", &stranger, &json!({}))
            .await
            .is_err()
    );
    assert!(
        core.operate("security.revoke", &owner, &json!({"source_id":source}))
            .await
            .is_err()
    );
    owner.allow_revoke = true;
    call(
        &core,
        &owner,
        "security.revoke",
        json!({"source_id":source}),
    )
    .await;
    assert!(
        core.operate("source.read", &owner, &json!({"source_id":source}))
            .await
            .is_err()
    );
    assert!(
        core.operate("hierarchy.expand", &owner, &json!({"cb_id":cb}))
            .await
            .is_err()
    );
    store.close().await;
}

#[tokio::test]
async fn late_native_tool_result_is_preserved_without_claiming_editor_saw_it() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let id = chain(&core).await;
    let ctx = context("owner", Some(&id));
    let request = json!({"messages":[{"role":"assistant","content":"searching","tool_calls":[{"id":"call-1","type":"function","function":{"name":"read","arguments":"{}"}}]}]});
    let first = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":request,"idempotency_key":"first"}),
    )
    .await;
    assert_eq!(first["open_calls"], json!(["call-1"]));
    let duplicate = core
        .operate(
            "source.ingest",
            &ctx,
            &json!({"request":request,"idempotency_key":"first"}),
        )
        .await
        .unwrap();
    assert_eq!(duplicate["evidence"]["replayed"], true);
    assert_eq!(duplicate["result"]["source_id"], first["source_id"]);
    let prepared = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":first["source_id"],"rules":[]}),
    )
    .await;
    let publish = json!({"job_id":prepared["job_id"],"expected_cut":0,"idempotency_key":"late"});
    assert_eq!(
        core.operate("cut.publish", &ctx, &publish)
            .await
            .unwrap_err()
            .code,
        "tool_group_open"
    );
    let late=call(&core,&ctx,"source.ingest",json!({"request":{"messages":[{"role":"tool","tool_call_id":"call-1","content":"actual tool output"}]}})).await;
    assert_eq!(late["source_id"], first["source_id"]);
    assert_eq!(late["open_calls"], json!([]));
    let result = call(&core, &ctx, "cut.publish", publish).await;
    let cut = call(&core, &ctx, "cut.get", json!({})).await;
    let cb = &cut["view"]["cbs"][0];
    assert_eq!(cb["body"], "searching\nactual tool output");
    assert_eq!(
        cb["editor_source_snapshot_ids"],
        json!([first["source_id"]])
    );
    assert_eq!(
        cb["source_ids"],
        json!([first["source_id"], late["part_source_id"]])
    );
    assert!(
        cb["ops"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op["origin"] == "post-snapshot-program")
    );
    let replay = call(
        &core,
        &ctx,
        "memory.replay",
        json!({"cb_id":result["cb_id"]}),
    )
    .await;
    assert_eq!(replay["body"], cb["body"]);
    assert_eq!(replay["body_matches_published_cb"], true);
    store.close().await;
}
