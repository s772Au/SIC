use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
async fn op(core: &Core, a: &OperationContext, name: &str, p: Value) -> Value {
    core.operate(name, a, &p)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))["result"]
        .clone()
}
async fn fixture() -> (Core, OperationContext, Value) {
    let core = Core::new(Store::memory().await.unwrap());
    let mut a = OperationContext {
        actor: "prod-host".into(),
        allow_write: true,
        ..Default::default()
    };
    a.chain_id = op(&core, &a, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .map(str::to_owned);
    let source=op(&core,&a,"source.ingest",json!({"request":{"messages":[{"role":"user","content":[{"type":"text","text":"A"},{"type":"text","text":"B"}]}]}})).await;
    let job=op(&core,&a,"edit.host.prepare",json!({"source_id":source["source_id"],"assignments":[{"editor":"a","unit_ids":["u1"]},{"editor":"b","unit_ids":["u2"]}]})).await;
    (core, a, job)
}
#[tokio::test]
async fn failed_multi_editor_job_cannot_be_resurrected_by_a_sibling_completion() {
    let (core, a, job) = fixture().await;
    let ca = json!({"job_id":job["job_id"],"editor":"a","claim_id":"claim-a"});
    let cb = json!({"job_id":job["job_id"],"editor":"b","claim_id":"claim-b"});
    op(&core, &a, "edit.host.claim", ca.clone()).await;
    op(&core, &a, "edit.host.claim", cb.clone()).await;
    let mut fail = ca;
    fail["reason"] = json!("executor outcome unknown");
    op(&core, &a, "edit.host.fail", fail).await;
    assert_eq!(
        core.operate("edit.host.submit", &a, &cb)
            .await
            .unwrap_err()
            .status,
        409
    );
    let current = op(&core, &a, "edit.host.get", json!({"job_id":job["job_id"]})).await;
    assert_eq!(current["status"], "failed");
    assert!(current["responses"]["b"].is_null());
    assert_eq!(current["editor_claims"]["b"], "claim-b");
    let mut settled = cb;
    settled["reason"] = json!("sibling cancellation settled");
    op(&core, &a, "edit.host.fail", settled).await;
    let current = op(&core, &a, "edit.host.get", json!({"job_id":job["job_id"]})).await;
    assert_eq!(current["status"], "failed");
    assert_eq!(current["editor_claims"], json!({}));
}

#[tokio::test]
async fn multi_editor_valid_deliveries_remain_scoped_atomic_and_replayable() {
    let (core, a, job) = fixture().await;
    let ca = json!({"job_id":job["job_id"],"editor":"a","claim_id":"claim-a"});
    let cb = json!({"job_id":job["job_id"],"editor":"b","claim_id":"claim-b"});
    op(&core, &a, "edit.host.claim", ca.clone()).await;
    op(&core, &a, "edit.host.claim", cb.clone()).await;
    let mut bad = cb.clone();
    bad["ops"] = json!([{"op":"REPLACE","target":"u1","content":"outside b"}]);
    assert!(core.operate("edit.host.submit", &a, &bad).await.is_err());
    let current = op(&core, &a, "edit.host.get", json!({"job_id":job["job_id"]})).await;
    assert_eq!(current["editor_claims"]["b"], "claim-b");
    assert!(current["responses"]["b"].is_null());
    op(&core, &a, "edit.host.submit", ca.clone()).await;
    let done = op(&core, &a, "edit.host.submit", cb.clone()).await;
    assert_eq!(done["status"], "ready");
    let published = op(
        &core,
        &a,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"multi"}),
    )
    .await;
    let replay = op(
        &core,
        &a,
        "memory.replay",
        json!({"cb_id":published["cb_id"]}),
    )
    .await;
    assert_eq!(replay["body"], "A\nB");
    assert_eq!(replay["body_matches_published_cb"], true);
    assert_eq!(op(&core, &a, "edit.host.claim", ca).await["execute"], false);
    assert_eq!(
        op(&core, &a, "edit.host.submit", cb).await["submission"],
        done["submission"]
    );
}

#[tokio::test]
async fn host_prepare_replay_rejects_changed_assignment_authority() {
    let (core, a, job) = fixture().await;
    let same = json!({"source_id":job["source_id"],"assignments":job["assignments"]});
    assert_eq!(op(&core, &a, "edit.host.prepare", same).await, job);
    let changed = json!({"source_id":job["source_id"],"assignments":[
        {"editor":"a","unit_ids":["u2"]},{"editor":"b","unit_ids":["u1"]}]});
    assert_eq!(
        core.operate("edit.host.prepare", &a, &changed)
            .await
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(
        op(&core, &a, "edit.host.get", json!({"job_id":job["job_id"]})).await,
        job
    );
    assert_eq!(
        op(
            &core,
            &a,
            "edit.host.prepare",
            json!({"source_id":job["source_id"]})
        )
        .await,
        job
    );
}

#[tokio::test]
async fn host_job_revocation_checks_only_its_own_frozen_dependencies() {
    let (core, mut a, first) = fixture().await;
    a.allow_revoke = true;
    for editor in ["a", "b"] {
        let claim = json!({"job_id":first["job_id"],"editor":editor,"claim_id":editor});
        op(&core, &a, "edit.host.claim", claim.clone()).await;
        op(&core, &a, "edit.host.submit", claim).await;
    }
    op(
        &core,
        &a,
        "cut.publish",
        json!({"job_id":first["job_id"],"expected_cut":0,"idempotency_key":"first"}),
    )
    .await;
    let source = op(
        &core,
        &a,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"independent B"}]}}),
    )
    .await;
    let second = op(
        &core,
        &a,
        "edit.host.prepare",
        json!({"source_id":source["source_id"]}),
    )
    .await;
    op(
        &core,
        &a,
        "security.revoke",
        json!({"source_id":first["source_id"]}),
    )
    .await;
    op(
        &core,
        &a,
        "edit.candidate.get",
        json!({"job_id":second["job_id"]}),
    )
    .await;
    op(
        &core,
        &a,
        "edit.host.get",
        json!({"job_id":second["job_id"]}),
    )
    .await;
    let claim = json!({"job_id":second["job_id"],"editor":"editor-1","claim_id":"independent"});
    op(&core, &a, "edit.host.claim", claim.clone()).await;
    op(&core, &a, "edit.host.submit", claim.clone()).await;
    op(
        &core,
        &a,
        "security.revoke",
        json!({"source_id":second["source_id"]}),
    )
    .await;
    for operation in [
        "edit.host.get",
        "edit.host.claim",
        "edit.host.submit",
        "edit.host.fail",
    ] {
        assert_eq!(
            core.operate(operation, &a, &claim)
                .await
                .unwrap_err()
                .status,
            403,
            "{operation}"
        );
    }
}
