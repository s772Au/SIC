use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    scheduler::{EditorMode, SchedulerPolicy},
    storage::Store,
};
async fn op(core: &Core, a: &OperationContext, name: &str, p: Value) -> Value {
    core.operate(name, a, &p).await.unwrap()["result"].clone()
}
fn policy(min_level: u32) -> Value {
    serde_json::to_value(SchedulerPolicy {
        editor_mode: EditorMode::Host,
        min_level,
        max_level: 3,
        level_batch_sizes: [(2, 2), (3, 2)].into(),
        max_sources_per_wb: 1,
        max_actions: 256,
        max_model_calls: 0,
        ..Default::default()
    })
    .unwrap()
}
async fn setup() -> (Core, OperationContext) {
    let core = Core::new(Store::memory().await.unwrap());
    let mut a = OperationContext {
        actor: "host-scheduler".into(),
        allow_write: true,
        allow_model: false,
        ..Default::default()
    };
    a.chain_id = op(&core, &a, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .map(str::to_owned);
    (core, a)
}
async fn ingest(core: &Core, a: &OperationContext, text: &str) {
    op(
        core,
        a,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
}
async fn drain(core: &Core, a: &OperationContext, min: u32) -> Vec<Value> {
    let mut publications = Vec::new();
    for _ in 0..100 {
        let report = op(core, a, "runtime.schedule", policy(min)).await;
        assert!(report["failure"].is_null(), "{report}");
        assert_eq!(report["model_calls_observed"], 0);
        assert_eq!(report["model_calls_reserved_unknown"], 0);
        publications.extend(report["published"].as_array().unwrap().iter().cloned());
        let Some(job) = report["host_jobs"].as_array().unwrap().first() else {
            return publications;
        };
        let claim = json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":format!("dispatch-{}",job["job_id"].as_str().unwrap())});
        let assigned = op(core, a, "edit.host.claim", claim.clone()).await;
        assert_eq!(assigned["execute"], true);
        assert_eq!(
            assigned["assigned_unit_ids"],
            job["assignments"][0]["unit_ids"]
        );
        assert_eq!(
            op(core, a, "edit.host.claim", claim.clone()).await["execute"],
            false
        );
        let get = op(
            core,
            a,
            "edit.host.get",
            json!({"job_id":job["job_id"],"editor":"editor-1"}),
        )
        .await;
        assert_eq!(get["fresh"], true);
        assert_eq!(get["source_digest"], assigned["source_digest"]);
        let repeated = op(core, a, "runtime.schedule", policy(min)).await;
        assert_eq!(repeated["host_jobs"][0]["job_id"], job["job_id"]);
        assert!(repeated["published"].as_array().unwrap().is_empty());
        let mut response = claim.clone();
        response["ops"] = json!([]);
        response["pvi"] = json!({});
        let done = op(core, a, "edit.host.submit", response.clone()).await;
        assert_eq!(done["status"], "ready");
        assert_eq!(
            op(core, a, "edit.host.submit", response.clone()).await["submission"],
            done["submission"]
        );
        response["pvi"] = json!({"I":{"changed":"replay"}});
        assert_eq!(
            core.operate("edit.host.submit", a, &response)
                .await
                .unwrap_err()
                .status,
            409
        );
    }
    panic!("scheduler did not converge");
}
#[tokio::test]
async fn host_mode_grows_three_real_levels_and_adopts_using_no_core_provider_or_profiles() {
    let (core, a) = setup().await;
    let mut published = Vec::new();
    for i in 0..13 {
        ingest(&core, &a, &format!("fact-{i}")).await;
        published.extend(drain(&core, &a, 1).await);
    }
    assert!(published.iter().any(|p| p["level"] == 3));
    assert!(published.iter().all(|p| p["provider_called"] == false));
    let chain = op(&core, &a, "chain.get", json!({})).await;
    assert!(
        chain["contexts"]["main"]["levels"]["3"]["cbs"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
    assert_eq!(chain["role_views"]["worker"]["level"], 3);
    assert_eq!(chain["role_views"]["editor"]["level"], 3);
    for p in published {
        let replay = op(&core, &a, "memory.replay", json!({"cb_id":p["cb_id"]})).await;
        assert_eq!(replay["body_matches_published_cb"], true);
    }
}
#[tokio::test]
async fn higher_only_never_consumes_pending_volatile_source_or_new_first_order_material() {
    let (core, a) = setup().await;
    ingest(&core, &a, "raw").await;
    let s =
        op(&core, &a, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]["pending"]
            [0]
        .clone();
    let j = op(
        &core,
        &a,
        "edit.program",
        json!({"source_id":s,"rules":[],"pvi":{"V":["next-WB"]}}),
    )
    .await;
    op(
        &core,
        &a,
        "cut.publish",
        json!({"job_id":j["job_id"],"expected_cut":0,"idempotency_key":"V"}),
    )
    .await;
    let before = op(&core, &a, "chain.get", json!({})).await;
    let report = op(&core, &a, "runtime.schedule", policy(2)).await;
    assert!(report["failure"].is_null());
    assert!(report["host_jobs"].as_array().unwrap().is_empty());
    assert_eq!(
        op(&core, &a, "chain.get", json!({})).await["contexts"]["main"]["levels"]["1"]["pending"],
        before["contexts"]["main"]["levels"]["1"]["pending"]
    );
}
#[tokio::test]
async fn claims_survive_reopen_never_redispatch_and_stale_or_unknown_delivery_never_overwrites() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("host.db");
    let core = Core::new(Store::create(&path).await.unwrap());
    let mut a = OperationContext {
        actor: "persistent-host".into(),
        allow_write: true,
        ..Default::default()
    };
    a.chain_id = op(&core, &a, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .map(str::to_owned);
    ingest(&core, &a, "one").await;
    let report = op(&core, &a, "runtime.schedule", policy(1)).await;
    let job = report["host_jobs"][0].clone();
    let c = json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":"stable-dispatch"});
    assert_eq!(
        op(&core, &a, "edit.host.claim", c.clone()).await["execute"],
        true
    );
    drop(core);
    let core = Core::new(Store::open(&path).await.unwrap());
    assert_eq!(
        op(&core, &a, "edit.host.claim", c.clone()).await["execute"],
        false
    );
    let mut other = c.clone();
    other["claim_id"] = json!("second-dispatch");
    assert_eq!(
        core.operate("edit.host.claim", &a, &other)
            .await
            .unwrap_err()
            .status,
        409
    );
    ingest(&core, &a, "new input invalidates the old snapshot").await;
    assert_eq!(
        op(&core, &a, "edit.host.get", json!({"job_id":job["job_id"]})).await["fresh"],
        false
    );
    let mut done = c.clone();
    done["ops"] = json!([]);
    done["pvi"] = json!({});
    assert_eq!(
        core.operate("edit.host.submit", &a, &done)
            .await
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(op(&core, &a, "chain.get", json!({})).await["cut_seq"], 0);
    let mut failure = c.clone();
    failure["reason"] = json!("lost external outcome");
    op(&core, &a, "edit.host.fail", failure).await;
    let blocked = op(&core, &a, "runtime.schedule", policy(1)).await;
    assert_eq!(blocked["host_jobs"][0]["job_id"], job["job_id"]);
    assert_eq!(blocked["host_jobs"][0]["status"], "failed");
    assert_eq!(op(&core, &a, "edit.host.claim", c).await["execute"], false);
    assert_eq!(
        core.operate("edit.submit", &a, &done)
            .await
            .unwrap_err()
            .status,
        403
    );
}

#[tokio::test]
async fn higher_volatile_pending_is_preserved_and_cannot_be_skipped_to_form_a_later_wb() {
    let (core, a) = setup().await;
    for i in 0..2 {
        ingest(&core, &a, &format!("before-{i}")).await;
        let c = op(&core, &a, "chain.get", json!({})).await;
        let j = op(
            &core,
            &a,
            "edit.program",
            json!({"source_id":c["contexts"]["main"]["levels"]["1"]["pending"][0],"rules":[]}),
        )
        .await;
        op(&core,&a,"cut.publish",json!({"job_id":j["job_id"],"expected_cut":c["cut_seq"],"idempotency_key":format!("low-{i}")})).await;
    }
    let c = op(&core, &a, "chain.get", json!({})).await;
    let h = op(
        &core,
        &a,
        "hierarchy.form",
        json!({"level":2,"cb_ids":c["contexts"]["main"]["levels"]["1"]["cbs"]}),
    )
    .await;
    let j = op(
        &core,
        &a,
        "edit.program",
        json!({"source_id":h["source_id"],"rules":[],"pvi":{"V":["legacy higher pending"]}}),
    )
    .await;
    op(
        &core,
        &a,
        "cut.publish",
        json!({"job_id":j["job_id"],"expected_cut":c["cut_seq"],"idempotency_key":"high-V"}),
    )
    .await;
    let before = op(&core, &a, "chain.get", json!({})).await;
    let report = op(&core, &a, "runtime.schedule", policy(2)).await;
    assert!(report["failure"].is_null(), "{report}");
    assert_eq!(report["wait"]["code"], "higher_volatile_input");
    let after = op(&core, &a, "chain.get", json!({})).await;
    assert_eq!(after["contexts"], before["contexts"]);
    assert_eq!(after["cut_seq"], before["cut_seq"]);
}
#[tokio::test]
async fn ready_completion_cannot_publish_after_a_new_input_epoch_and_concurrent_plans_share_one_job()
 {
    let (core, a) = setup().await;
    ingest(&core, &a, "first").await;
    let p = policy(1);
    let (left, right) = tokio::join!(
        core.operate("runtime.schedule", &a, &p),
        core.operate("runtime.schedule", &a, &p)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(
        left["result"]["host_jobs"][0]["job_id"],
        right["result"]["host_jobs"][0]["job_id"]
    );
    let id = left["result"]["host_jobs"][0]["job_id"].clone();
    let claim = json!({"job_id":id,"editor":"editor-1","claim_id":"concurrent-only-once"});
    op(&core, &a, "edit.host.claim", claim.clone()).await;
    op(&core, &a, "edit.host.submit", claim).await;
    ingest(&core, &a, "new epoch after a valid submission").await;
    let cut = op(&core, &a, "chain.get", json!({})).await["cut_seq"].clone();
    assert_eq!(
        core.operate(
            "cut.publish",
            &a,
            &json!({"job_id":id,"expected_cut":cut,"idempotency_key":"stale-ready"})
        )
        .await
        .unwrap_err()
        .status,
        409
    );
    assert_eq!(op(&core, &a, "chain.get", json!({})).await["cut_seq"], cut);
}

#[tokio::test]
async fn newly_published_host_higher_v_halts_the_same_scheduler_pass() {
    let (core,a)=setup().await;
    for index in 0..2 {
        ingest(&core,&a,&format!("fact-{index}")).await;
        let state=op(&core,&a,"chain.get",json!({})).await;
        let job=op(&core,&a,"edit.program",json!({"source_id":state["contexts"]["main"]["levels"]["1"]["pending"][0],"rules":[]})).await;
        op(&core,&a,"cut.publish",json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":format!("seed-{index}")})).await;
    }
    let planned=op(&core,&a,"runtime.schedule",policy(2)).await;
    let job=&planned["host_jobs"][0];
    let mut delivery=json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":"new-higher-v"});
    op(&core,&a,"edit.host.claim",delivery.clone()).await;
    delivery["pvi"]=json!({"V":["preserve until an authorized next boundary"]});
    op(&core,&a,"edit.host.submit",delivery).await;
    let published=op(&core,&a,"runtime.schedule",policy(2)).await;
    assert_eq!(published["published"].as_array().unwrap().len(),1);
    assert_eq!(published["wait"]["code"],"higher_volatile_input");
    assert!(published["host_jobs"].as_array().unwrap().is_empty());
    assert_eq!(op(&core,&a,"chain.get",json!({})).await["contexts"]["main"]["levels"]["2"]["pending"].as_array().unwrap().len(),1);
}
