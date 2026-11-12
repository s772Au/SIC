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
async fn count_objects(core: &Core, kind: &str) -> i64 {
    let mut tx = core.store.begin().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE kind=?")
        .bind(kind)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    count
}

async fn prepare_publish(core: &Core, ctx: &OperationContext, source: &str, key: &str) -> String {
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
async fn revoked_source_blocks_direct_and_derived_entry_points_without_side_effects() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let secret = ingest(&core, &ctx, "SECRET-REVOKED").await;
    let visible = ingest(&core, &ctx, "VISIBLE-KEPT").await;
    call(
        &core,
        &ctx,
        "wb.form",
        json!({"source_ids":[secret, visible]}),
    )
    .await;
    let first = prepare_publish(&core, &ctx, &secret, "revoke-first").await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":2,"cb_ids":[first]}),
    )
    .await;
    let second = prepare_publish(
        &core,
        &ctx,
        higher["source_id"].as_str().unwrap(),
        "revoke-second",
    )
    .await;
    let cuts_before = count_objects(&core, "cut").await;

    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":secret,"reason":"fixture revocation"}),
    )
    .await;

    let forbidden = [
        ("source.read", json!({"source_id":secret})),
        ("source.text", json!({"source_id":secret,"unit_id":"u1"})),
        (
            "edit.prepare",
            json!({"source_id":secret,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
        ),
        ("hierarchy.expand", json!({"cb_id":first})),
        ("hierarchy.expand", json!({"cb_id":second})),
        ("cut.get", json!({})),
    ];
    for (op, payload) in forbidden {
        let error = core.operate(op, &ctx, &payload).await.unwrap_err();
        assert_eq!(
            error.code, "forbidden",
            "{op} must not serve revoked or derived material: {error:?}"
        );
    }
    // Search may return no hit, but it must never echo revoked text.
    let search = core
        .operate("memory.search", &ctx, &json!({"query":"SECRET-REVOKED"}))
        .await;
    match search {
        Err(error) => assert_eq!(error.code, "forbidden"),
        Ok(value) => {
            let text = serde_json::to_string(&value).unwrap();
            assert!(!text.contains("SECRET-REVOKED"), "{text}");
        }
    }
    // Status reads now stay available but redacted; content and derived reads
    // on the revoked chain remain blocked.
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let state_text = serde_json::to_string(&state).unwrap();
    assert!(!state_text.contains("SECRET-REVOKED"), "{state_text}");
    assert!(state["jobs"].as_object().is_some());
    assert_eq!(
        count_objects(&core, "cut").await,
        cuts_before,
        "rejected operations must not archive a new cut"
    );
    let visible_read = call(&core, &ctx, "source.read", json!({"source_id":visible})).await;
    assert_eq!(visible_read["source"]["units"][0]["text"], "VISIBLE-KEPT");
    // The raw object and the revocation record are both retained immutably.
    let mut tx = core.store.begin().await.unwrap();
    let objects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE id=?")
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let revocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM revocations WHERE id=?")
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(objects, 1, "the archive is not physically erased");
    assert_eq!(revocations, 1, "exactly one immutable revocation record");
}
