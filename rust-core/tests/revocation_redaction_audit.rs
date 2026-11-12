use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::{self, Store},
};

const REVOKED: &str = "REVOKED-SENTINEL-AAA";
const CANDIDATE: &str = "CANDIDATE-QUOTE-BBB";

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
async fn probe(
    core: &Core,
    ctx: &OperationContext,
    op: &str,
    payload: Value,
) -> Result<Value, String> {
    core.operate(op, ctx, &payload)
        .await
        .map(|response| response["result"].clone())
        .map_err(|error| format!("{}: {}", error.code, error.message))
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
async fn program_publish(
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
async fn no_successful_response_echoes_revoked_text_and_redaction_is_explicit() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context("owner", None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context("owner", Some(&chain));
    let secret = ingest(&core, &ctx, &format!("{REVOKED} payload")).await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[secret]})).await;
    let (cb, secret_job) = program_publish(&core, &ctx, &secret, "redact-cb").await;

    // An unrelated open job whose candidate quotes a different sentinel: the
    // status redaction still removes candidate text, which is conservative.
    let other = ingest(&core, &ctx, "other source").await;
    call(&core, &ctx, "wb.form", json!({"source_ids":[other]})).await;
    let other_job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":other,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &core,
        &ctx,
        "edit.submit",
        json!({"job_id":other_job,"editor":"e1",
               "ops":[{"op":"REPLACE","target":"u1","content":format!("{CANDIDATE} text")}],
               "pvi":{}}),
    )
    .await;

    let before = call(&core, &ctx, "chain.get", json!({})).await;
    let pack_before = call(&core, &ctx, "experiment.pack", json!({})).await;
    assert!(
        serde_json::to_string(&before).unwrap().contains(REVOKED),
        "control: the frozen snapshot is visible before revocation"
    );

    call(
        &core,
        &ctx,
        "security.revoke",
        json!({"source_id":secret,"reason":"fixture revocation"}),
    )
    .await;

    let state = call(&core, &ctx, "chain.get", json!({})).await;
    let text = serde_json::to_string(&state).unwrap();
    assert!(!text.contains(REVOKED), "{text}");
    assert!(!text.contains(CANDIDATE), "{text}");
    assert_eq!(
        state["jobs"][&secret_job]["source_snapshot"]["redacted"],
        "revocation"
    );
    assert_eq!(
        state["jobs"][&other_job]["source_snapshot"]["redacted"],
        "revocation"
    );
    assert_eq!(
        state["contexts"]["main"]["levels"]["1"]["instant"]["redacted"],
        "revocation"
    );

    let probes = [
        ("source.read", json!({"source_id":secret})),
        ("source.text", json!({"source_id":secret,"unit_id":"u1"})),
        (
            "edit.prepare",
            json!({"source_id":secret,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
        ),
        ("edit.validate", json!({"job_id":secret_job})),
        ("edit.candidate.get", json!({"job_id":secret_job})),
        ("cut.get", json!({})),
        ("hierarchy.expand", json!({"cb_id":cb})),
        ("hierarchy.timeline", json!({"level":1,"kind":"cb"})),
        ("memory.search", json!({"query":REVOKED})),
        ("memory.page", json!({})),
        ("memory.replay", json!({})),
        ("legacy.status", json!({})),
        ("board.history", json!({})),
        ("hierarchy.get", json!({})),
        ("decision.get", json!({})),
        ("config.get", json!({})),
        ("accounting.get", json!({})),
        ("tools.status", json!({})),
        ("tools.tasks", json!({})),
        ("peer.tasks", json!({})),
        ("experiment.pack", json!({})),
        ("experiment.verify", json!({"pack":pack_before})),
        ("runtime.budget", json!({"wb_source_id":secret,"budget":1})),
    ];
    for (op, payload) in probes {
        match probe(&core, &ctx, op, payload).await {
            Ok(value) => {
                let text = serde_json::to_string(&value).unwrap();
                assert!(
                    !text.contains(REVOKED),
                    "{op} returned revoked text after revocation"
                );
            }
            Err(error) => {
                assert!(
                    !error.contains(REVOKED),
                    "{op} error echoed revoked text: {error}"
                );
            }
        }
    }
    // The archive still stores the raw bytes; only authorized access is blocked.
    let mut tx = store.begin().await.unwrap();
    let stored = storage::get_object(&mut tx, &secret, Some("source")).await;
    assert!(stored.is_err(), "direct object read must also be revoked");
    tx.commit().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    let raw: String = sqlx::query_scalar("SELECT data FROM objects WHERE id=?")
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(raw.contains(REVOKED), "physical erasure is not claimed");
    store.close().await;
}
