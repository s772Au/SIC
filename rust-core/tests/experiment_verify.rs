use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
        chain_id: chain,
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
async fn publish(core: &Core, ctx: &OperationContext, text: &str, key: &str) {
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
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await;
}

#[tokio::test]
async fn experiment_verify_recomputes_digests_and_reports_stale_cut() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    publish(&core, &ctx, "packed material", "verify-one").await;
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    let verified = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(verified["ok"], true, "{verified}");
    assert_eq!(verified["cut_seq"]["matches"], true);
    assert!(verified["mismatches"].as_array().unwrap().is_empty());
    assert_eq!(verified["verification"]["metadata_only"], true);
    assert!(pack["manifest_digest"].is_string());
    let repeated = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert_eq!(
        pack["manifest_digest"], repeated["manifest_digest"],
        "the manifest is stable across pack calls because created_at is excluded"
    );
    let mut first = pack.clone();
    first.as_object_mut().unwrap().remove("created_at");
    let mut second = repeated.clone();
    second.as_object_mut().unwrap().remove("created_at");
    assert_eq!(first, second);

    let mut tampered = pack.clone();
    tampered["sources"][0]["digest"] = json!("00");
    let tampered_result = call(&core, &ctx, "experiment.verify", json!({"pack":tampered})).await;
    assert_eq!(tampered_result["ok"], false);
    assert!(
        tampered_result["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "digest_changed")
    );
    let mut privacy = pack.clone();
    privacy["privacy"]["source_text_included"] = json!(true);
    let privacy_result = call(&core, &ctx, "experiment.verify", json!({"pack":privacy})).await;
    assert_eq!(privacy_result["ok"], false);
    assert!(
        privacy_result["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "manifest_changed")
    );

    let mut missing = pack.clone();
    missing["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source_id":"source_missing","digest":"00"}));
    let missing_result = call(&core, &ctx, "experiment.verify", json!({"pack":missing})).await;
    assert_eq!(missing_result["ok"], false);
    assert!(
        missing_result["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "missing")
    );

    // A later cut does not invalidate the old digests; staleness is reported
    // separately from tampering.
    publish(&core, &ctx, "later material", "verify-two").await;
    let stale = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(stale["ok"], true);
    assert_eq!(stale["cut_seq"]["matches"], false);

    call(
        &core,
        &ctx,
        "config.update",
        json!({"task_goal":"changed after pack"}),
    )
    .await;
    let config_changed = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(config_changed["ok"], false);
    assert!(
        config_changed["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "config_changed"),
        "{config_changed}"
    );
    store.close().await;
}

#[tokio::test]
async fn experiment_verify_detects_raw_object_data_and_digest_tampering() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    publish(&core, &ctx, "raw digest material", "verify-raw").await;
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    let cb_id = pack["blocks"][0]["cb_id"].as_str().unwrap().to_owned();
    let mut tx = store.begin().await.unwrap();
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='objects'",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    for trigger in triggers {
        sqlx::query(&format!("DROP TRIGGER IF EXISTS \"{trigger}\""))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let original: String = sqlx::query_scalar("SELECT data FROM objects WHERE id=? AND kind='cb'")
        .bind(&cb_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE objects SET data=? WHERE id=? AND kind='cb'")
        .bind(r#"{"tampered":true}"#)
        .bind(&cb_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let data_tampered = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(data_tampered["ok"], false);
    assert!(
        data_tampered["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "stored_digest_mismatch"),
        "{data_tampered}"
    );

    let mut tx = store.begin().await.unwrap();
    sqlx::query("UPDATE objects SET data=?,digest='00' WHERE id=? AND kind='cb'")
        .bind(&original)
        .bind(&cb_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let digest_tampered = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(digest_tampered["ok"], false);
    assert!(
        digest_tampered["mismatches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reason"] == "stored_digest_mismatch"),
        "{digest_tampered}"
    );
    store.close().await;
}
#[tokio::test]
async fn experiment_pack_verifies_after_store_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("experiment-pack.sqlite");
    let store = Store::create_with_connections(&path, 4).await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    publish(&core, &ctx, "durable pack material", "pack-reopen").await;
    let pack = call(&core, &ctx, "experiment.pack", json!({})).await;
    drop(core);
    store.close().await;

    let reopened = Store::open_with_connections(&path, 4).await.unwrap();
    let core = Core::new(reopened.clone());
    let verified = call(&core, &ctx, "experiment.verify", json!({"pack":pack})).await;
    assert_eq!(verified["ok"], true, "{verified}");
    assert_eq!(verified["cut_seq"]["matches"], true);
    assert_eq!(verified["verification"]["metadata_only"], true);
    reopened.close().await;
}
