use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::{self, Store},
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
async fn prepare_publish(
    core: &Core,
    ctx: &OperationContext,
    source: &str,
    key: &str,
) -> (String, String) {
    let prepared = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let job = prepared["job_id"].as_str().unwrap().to_owned();
    let state = call(core, ctx, "chain.get", json!({})).await;
    let cb = call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job,"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await["cb_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (cb, job)
}

#[tokio::test]
async fn unrelated_open_job_stays_usable_after_a_source_revocation() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut ctx = context("owner", Some(&chain));
    ctx.allow_model = true;
    let secret = ingest(&core, &ctx, "SECRET-REVOKED").await;
    let visible = ingest(&core, &ctx, "VISIBLE-KEPT").await;
    call(
        &core,
        &ctx,
        "wb.form",
        json!({"source_ids":[secret, visible]}),
    )
    .await;
    let (_, secret_job) = prepare_publish(&core, &ctx, &secret, "scope-first").await;

    // An unrelated WB and an open job prepared before the revocation.
    let alpha = ingest(&core, &ctx, "alpha fact").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[alpha]})).await;
    let alpha_job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":alpha,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tx = store.begin().await.unwrap();
    let cuts_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE kind='cut'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":secret,"reason":"fixture revocation"}),
    )
    .await;

    // A job whose frozen snapshot contains the revoked source is blocked on
    // every advertised job-scoped path, before the handler can act.
    for (op, payload) in [
        ("edit.validate", json!({"job_id":secret_job})),
        ("edit.candidate.get", json!({"job_id":secret_job})),
        ("edit.submit", json!({"job_id":secret_job})),
        ("edit.run", json!({"job_id":secret_job})),
        ("edit.candidate.update", json!({"job_id":secret_job})),
        (
            "cut.publish",
            json!({"job_id":secret_job,"expected_cut":0,"idempotency_key":"scope-secret"}),
        ),
    ] {
        let error = core.operate(op, &ctx, &payload).await.unwrap_err();
        assert_eq!(error.code, "forbidden", "{op}: {error:?}");
    }
    let prepare_error = core
        .operate(
            "edit.prepare",
            &ctx,
            &json!({"source_id":secret,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
        )
        .await
        .unwrap_err();
    assert_eq!(prepare_error.code, "forbidden");

    // The unrelated job is still usable end-to-end.
    let submitted = call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":alpha_job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":"alpha edited"}],
               "pvi":{}}),
    )
    .await;
    assert_eq!(submitted["status"], "ready");
    let published = call(
        &core,
        &ctx,
        "cut.publish",
        json!({"job_id":alpha_job,"expected_cut":cuts_before,
               "idempotency_key":"scope-alpha"}),
    )
    .await;
    let cb_id = published["cb_id"].as_str().unwrap().to_owned();
    let mut tx = store.begin().await.unwrap();
    let cb = storage::get_object(&mut tx, &cb_id, Some("cb"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let body = cb["body"].as_str().unwrap();
    assert!(body.contains("alpha edited"), "{body}");
    assert!(!body.contains("SECRET-REVOKED"), "{body}");
    let mut tx = store.begin().await.unwrap();
    let cuts_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE kind='cut'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cuts_after, cuts_before + 1);

    // Status reads stay available but are redacted: ids/metadata remain, while
    // frozen source text, candidates, I views and decision text do not.
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], cuts_after);
    let state_text = serde_json::to_string(&state).unwrap();
    assert!(!state_text.contains("SECRET-REVOKED"), "{state_text}");
    assert!(!state_text.contains("alpha edited"), "{state_text}");
    assert_eq!(
        state["jobs"][&secret_job]["source_snapshot"]["redacted"],
        "revocation"
    );
    assert_eq!(
        state["jobs"][&secret_job]["source_snapshot"]["source_ids"],
        json!([secret, visible])
    );
    assert_eq!(
        state["contexts"]["main"]["levels"]["1"]["instant"]["redacted"],
        "revocation"
    );
    assert_eq!(state["decision"]["redacted"], "revocation");
    assert!(state["decision"].get("revision").is_some());
    let hierarchy = call(&core, &ctx, "hierarchy.get", json!({})).await;
    assert_eq!(
        hierarchy["contexts"]["main"]["levels"]["1"]["instant"]["redacted"],
        "revocation"
    );
    assert_eq!(hierarchy["role_views"]["worker"]["context_id"], "main");
    let decision = call(&core, &ctx, "decision.get", json!({})).await;
    assert_eq!(decision["decision"]["redacted"], "revocation");
    call(&core, &ctx, "config.get", json!({})).await;
    call(&core, &ctx, "accounting.get", json!({})).await;
    let read = call(&core, &ctx, "source.read", json!({"source_id":alpha})).await;
    assert_eq!(read["source"]["units"][0]["text"], "alpha fact");
    store.close().await;
}
