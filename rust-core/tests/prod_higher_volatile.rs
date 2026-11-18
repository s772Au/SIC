use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    scheduler::{EditorMode, SchedulerPolicy},
    storage::Store,
};
async fn op(c: &Core, a: &OperationContext, n: &str, p: Value) -> Value {
    c.operate(n, a, &p).await.unwrap()["result"].clone()
}
async fn seed(c: &Core, a: &OperationContext, text: &str) {
    let s = op(
        c,
        a,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let j = op(
        c,
        a,
        "edit.program",
        json!({"source_id":s["source_id"],"rules":[]}),
    )
    .await;
    let seq = op(c, a, "chain.get", json!({})).await["cut_seq"].clone();
    op(
        c,
        a,
        "cut.publish",
        json!({"job_id":j["job_id"],"expected_cut":seq,"idempotency_key":format!("seed-{text}")}),
    )
    .await;
}
fn policy(flush: bool) -> Value {
    serde_json::to_value(SchedulerPolicy {
        editor_mode: EditorMode::Host,
        min_level: 2,
        max_level: 2,
        level_batch_sizes: [(2, 2)].into(),
        flush,
        max_model_calls: 0,
        ..Default::default()
    })
    .unwrap()
}
async fn fixture() -> (Core, OperationContext, Value) {
    let c = Core::new(Store::memory().await.unwrap());
    let mut a = OperationContext {
        actor: "higher-v".into(),
        allow_write: true,
        allow_revoke: true,
        ..Default::default()
    };
    a.chain_id = op(&c, &a, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .map(str::to_owned);
    seed(&c, &a, "old-A").await;
    seed(&c, &a, "old-B").await;
    let ids =
        op(&c, &a, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]["cbs"].clone();
    let h = op(&c, &a, "hierarchy.form", json!({"level":2,"cb_ids":ids})).await;
    let j = op(&c, &a, "edit.prepare", json!({"source_id":h["source_id"]})).await;
    op(&c,&a,"edit.submit",json!({"job_id":j["job_id"],"editor":"editor-1","ops":[{"op":"COMPRESS","target":"u1","content":"carry","lifecycle":"V"}],"pvi":{}})).await;
    let high = op(
        &c,
        &a,
        "cut.publish",
        json!({"job_id":j["job_id"],"expected_cut":2,"idempotency_key":"initial-high-V"}),
    )
    .await;
    (c, a, high)
}
#[tokio::test]
async fn same_level_volatile_source_joins_real_next_higher_wb_and_publishes_once() {
    let (c, a, first) = fixture().await;
    let v = first["volatile_source_id"].clone();
    let original = op(&c, &a, "source.read", json!({"source_id":v})).await;
    let waiting = op(&c, &a, "runtime.schedule", policy(false)).await;
    assert!(waiting["failure"].is_null());
    assert!(waiting["host_jobs"].as_array().unwrap().is_empty());
    seed(&c, &a, "new-C").await;
    seed(&c, &a, "new-D").await;
    let plan = op(&c, &a, "runtime.schedule", policy(false)).await;
    assert!(plan["failure"].is_null(), "{plan}");
    let jobs = plan["host_jobs"].as_array().unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "actual V and fresh lower CBs must make progress"
    );
    let job = &jobs[0];
    let texts: Vec<&str> = job["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, vec!["carry", "new-C", "new-D"]);
    assert_eq!(
        op(&c, &a, "source.read", json!({"source_id":v})).await,
        original,
        "immutable original V source changed"
    );
    let claim = json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":"actual-next-WB"});
    assert_eq!(
        op(&c, &a, "edit.host.claim", claim.clone()).await["execute"],
        true
    );
    op(&c, &a, "edit.host.submit", claim).await;
    let done = op(&c, &a, "runtime.schedule", policy(false)).await;
    assert!(done["failure"].is_null(), "{done}");
    assert_eq!(done["published"].as_array().unwrap().len(), 1);
    let cb = &done["published"][0]["cb_id"];
    assert_eq!(
        op(&c, &a, "memory.replay", json!({"cb_id":cb})).await["body"],
        "carry\nnew-C\nnew-D"
    );
    assert!(
        op(&c, &a, "chain.get", json!({})).await["contexts"]["main"]["levels"]["2"]["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        op(&c, &a, "runtime.schedule", policy(false)).await["host_jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_flush_can_form_a_v_only_next_wb_but_automatic_wait_keeps_worker_progress_open() {
    let (c, a, first) = fixture().await;
    let waiting = op(&c, &a, "runtime.schedule", policy(false)).await;
    assert_eq!(waiting["wait"]["code"], "higher_volatile_input");
    assert!(waiting["failure"].is_null());
    let plan = op(&c, &a, "runtime.schedule", policy(true)).await;
    assert!(plan["failure"].is_null(), "{plan}");
    let job = &plan["host_jobs"][0];
    assert_eq!(job["units"].as_array().unwrap().len(), 1);
    assert_eq!(job["units"][0]["text"], "carry");
    assert_eq!(
        job["source_snapshot"]["source_ids"],
        json!([first["volatile_source_id"]])
    );
    let claim = json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":"flush-v-only"});
    op(&c, &a, "edit.host.claim", claim.clone()).await;
    op(&c, &a, "edit.host.submit", claim).await;
    let done = op(&c, &a, "runtime.schedule", policy(true)).await;
    assert!(done["failure"].is_null(), "{done}");
    assert_eq!(done["published"].as_array().unwrap().len(), 1);
    assert_eq!(
        op(
            &c,
            &a,
            "memory.replay",
            json!({"cb_id":done["published"][0]["cb_id"]})
        )
        .await["body"],
        "carry"
    );
}

#[tokio::test]
async fn wrong_or_missing_v_binding_is_atomic_and_revocation_blocks_the_admitted_host_job() {
    let (c, a, first) = fixture().await;
    let v = first["volatile_source_id"].clone();
    seed(&c, &a, "new-C").await;
    seed(&c, &a, "new-D").await;
    let state = op(&c, &a, "chain.get", json!({})).await;
    let lower = state["contexts"]["main"]["levels"]["1"]["cbs"]
        .as_array()
        .unwrap();
    let members = json!([lower[2], lower[3]]);
    for bindings in [json!([]), json!([v, v]), json!(["foreign-source"])] {
        assert!(
            c.operate(
                "hierarchy.form",
                &a,
                &json!({"level":2,"cb_ids":members,"volatile_source_ids":bindings})
            )
            .await
            .is_err()
        );
        assert_eq!(
            op(&c, &a, "chain.get", json!({})).await["contexts"],
            state["contexts"]
        );
    }
    let plan = op(&c, &a, "runtime.schedule", policy(false)).await;
    let job = &plan["host_jobs"][0];
    op(&c, &a, "security.revoke", json!({"source_id":v})).await;
    assert_eq!(
        c.operate("edit.host.get", &a, &json!({"job_id":job["job_id"]}))
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        c.operate(
            "edit.host.claim",
            &a,
            &json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":"revoked-v"})
        )
        .await
        .unwrap_err()
        .status,
        403
    );
}

#[tokio::test]
async fn unclaimed_legacy_v_wb_is_superseded_without_rewriting_its_frozen_archive() {
    let (c, a, first) = fixture().await;
    let v = first["volatile_source_id"].clone();
    let legacy_id = "legacy-v-wb";
    let legacy = json!({"wb_id":legacy_id,"chain_id":a.chain_id,"context_id":"main","level":2,"source_id":v,"source_ids":[v],"unit_ids":["u1"],"members":[],"reason":"volatile-continuation"});
    let mut tx = c.store.begin().await.unwrap();
    sic_core_rust::storage::put_object(&mut tx, "wb", legacy_id, &legacy)
        .await
        .unwrap();
    let mut state =
        sic_core_rust::storage::get_chain(&mut tx, a.chain_id.as_deref().unwrap(), &a.actor)
            .await
            .unwrap();
    state["contexts"]["main"]["levels"]["2"]["wbs"]
        .as_array_mut()
        .unwrap()
        .push(json!(legacy_id));
    sic_core_rust::storage::save_chain(&mut tx, &state)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    seed(&c, &a, "new-C").await;
    seed(&c, &a, "new-D").await;
    let plan = op(&c, &a, "runtime.schedule", policy(false)).await;
    assert!(plan["failure"].is_null(), "{plan}");
    assert_eq!(plan["host_jobs"][0]["units"].as_array().unwrap().len(), 3);
    assert!(op(&c,&a,"chain.get",json!({})).await["contexts"]["main"]["levels"]["2"]["superseded_wbs"][legacy_id].is_string());
    let mut tx = c.store.begin().await.unwrap();
    assert_eq!(
        sic_core_rust::storage::get_object(&mut tx, legacy_id, Some("wb"))
            .await
            .unwrap(),
        legacy
    );
}
