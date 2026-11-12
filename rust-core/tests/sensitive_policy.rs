use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::{self, Store},
};

fn context(actor: &str, chain: Option<&str>, allow_revoke: bool) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke,
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
async fn publish_program(core: &Core, ctx: &OperationContext, source: &str, key: &str) -> String {
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
async fn sensitivity_policy_is_explicit_and_ordinary_delete_is_not_a_security_path() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(
        &core,
        &context("owner", None, false),
        "chain.create",
        json!({}),
    )
    .await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain), true);
    let secret = ingest(&core, &ctx, "SENSITIVE-7c1 payload").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[secret]})).await;
    let secret_cb = publish_program(&core, &ctx, &secret, "sensitive-l1").await;
    let higher = call(
        &core,
        &ctx,
        "hierarchy.form",
        json!({"level":2,"cb_ids":[secret_cb]}),
    )
    .await;
    let higher_cb = publish_program(
        &core,
        &ctx,
        higher["source_id"].as_str().unwrap(),
        "sensitive-l2",
    )
    .await;

    // Classification needs the separate security authorization.
    let denied = core
        .operate(
            "security.classify",
            &context("owner", Some(&chain), false),
            &json!({"source_id":secret}),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code, "revoke_denied");
    // Only the declared access policy is implemented; erasure is refused.
    let erasure = core
        .operate(
            "security.classify",
            &ctx,
            &json!({"source_id":secret,"policy":"physical_erase"}),
        )
        .await
        .unwrap_err();
    assert_eq!(erasure.code, "unsupported");
    let classified = call(
        &core,
        &ctx,
        "security.classify",
        json!({"source_id":secret}),
    )
    .await;
    assert_eq!(classified["policy"], "access_revoke");
    assert_eq!(classified["physical_erasure"], false);
    let duplicate = core
        .operate(
            "security.classify",
            &ctx,
            &json!({"source_id":secret,"policy":"access_revoke"}),
        )
        .await
        .unwrap_err();
    assert!(
        duplicate.message.contains("already has an immutable"),
        "{duplicate:?}"
    );

    // Ordinary Editor DELETE changes the CB body only; the raw archive and the
    // absence of a security record prove it is not a security deletion.
    let deletable = ingest(&core, &ctx, "deletable text").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[deletable]})).await;
    let prepared = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":deletable,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await;
    let job = prepared["job_id"].as_str().unwrap().to_owned();
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":job,"editor":"e1",
               "ops":[{"op":"DELETE","target":"u1","reason":"ordinary edit"}],"pvi":{}}),
    )
    .await;
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":"delete-cb"}),
    )
    .await;
    let archive = call(&core, &ctx, "source.read", json!({"source_id":deletable})).await;
    assert_eq!(
        archive["source"]["units"][0]["text"], "deletable text",
        "ordinary DELETE must not rewrite the raw archive"
    );

    // An unclassified revocation keeps the historical response shape and does
    // not pretend to be classification.
    let plain = call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":deletable,"reason":"plain revoke"}),
    )
    .await;
    assert_eq!(plain["access_revoked"], true);
    assert_eq!(plain["physical_erasure"], false);
    assert!(plain["sensitivity"].is_null());

    // The classified source records its policy in the revocation result.
    let revoked = call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":secret,"reason":"classified revoke"}),
    )
    .await;
    assert_eq!(revoked["sensitivity"], "sensitive");
    assert_eq!(revoked["policy"], "access_revoke");
    assert_eq!(revoked["physical_erasure"], false);

    // Derived and direct reads are blocked; status is redacted.
    for (op, payload) in [
        ("source.read", json!({"source_id":secret})),
        ("hierarchy.expand", json!({"cb_id":secret_cb})),
        ("hierarchy.expand", json!({"cb_id":higher_cb})),
    ] {
        let error = core.operate(op, &ctx, &payload).await.unwrap_err();
        assert_eq!(error.code, "forbidden", "{op}");
    }
    let status = call(&core, &ctx, "chain.get", json!({})).await;
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("SENSITIVE-7c1")
    );
    // The raw bytes and the immutable policy row remain; no erasure is claimed.
    let mut tx = store.begin().await.unwrap();
    let raw: String = sqlx::query_scalar("SELECT data FROM objects WHERE id=?")
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let policy: String = sqlx::query_scalar("SELECT record FROM sensitivity WHERE id=?")
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(raw.contains("SENSITIVE-7c1"));
    assert!(policy.contains("access_revoke"));
    let update = sqlx::query("UPDATE sensitivity SET record='changed' WHERE id=?")
        .bind(&secret)
        .execute(&mut *tx)
        .await;
    assert!(update.is_err(), "sensitivity rows are immutable");
    tx.commit().await.unwrap();
    assert!(
        storage::get_object(&mut store.begin().await.unwrap(), &secret, Some("source"))
            .await
            .is_err()
    );
    store.close().await;
}
