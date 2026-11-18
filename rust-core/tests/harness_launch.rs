use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    harness_lifecycle::{HarnessGrant, HarnessRole, HumanIntent, LaunchIdentity},
    storage::{self, Store},
};
fn auth() -> OperationContext {
    OperationContext {
        actor: "launch-fixture".into(),
        allow_write: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn independent_hosts_racing_one_logical_intent_receive_at_most_one_permit() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("racing-launch.db");
    let a = auth();
    let first = Store::create(&path).await.unwrap();
    let second = Store::open(&path).await.unwrap();
    let one = Core::new(first.clone());
    let two = Core::new(second.clone());
    let prepared = intent(&one, &a, "intent", "race-call", json!({"work":"same"})).await;
    let id = prepared["object_id"].as_str().unwrap();
    let left = request(
        &one,
        &a,
        id,
        "claim-left",
        json!({"kind":"job.claim","claim_id":"left"}),
    )
    .await;
    let right = request(
        &two,
        &a,
        id,
        "claim-right",
        json!({"kind":"job.claim","claim_id":"right"}),
    )
    .await;
    let (left, right) = tokio::join!(
        one.operate("harness.observe", &a, &left),
        two.operate("harness.observe", &a, &right)
    );
    let permits = [left, right]
        .into_iter()
        .filter(|r| {
            r.as_ref()
                .is_ok_and(|v| v["result"]["execution_permit"] == true)
        })
        .count();
    assert_eq!(permits, 1);
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn crash_after_claim_before_executor_is_unknown_and_legacy_broker_cannot_redispatch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("before-dispatch.db");
    let store = Store::create(&path).await.unwrap();
    let c = Core::new(store.clone());
    let mut a = auth();
    a.chain_id = Some(
        c.operate("chain.create", &a, &json!({})).await.unwrap()["result"]["chain_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    let prepared = intent(
        &c,
        &a,
        "intent",
        "call-before-dispatch",
        json!({"task":"one"}),
    )
    .await;
    let id = prepared["object_id"].as_str().unwrap().to_owned();
    let claim = observe(
        &c,
        &a,
        &id,
        "claim",
        json!({"kind":"job.claim","claim_id":"crashed-claim"}),
    )
    .await;
    assert_eq!(claim["execution_permit"], true);
    // No executor invocation happened. A restart must not assume the opposite.
    let task_id = prepared["task_id"].clone();
    assert!(
        c.operate(
            "tool.claim",
            &a,
            &json!({"task_id":task_id,"claim_id":"legacy"})
        )
        .await
        .is_err()
    );
    assert!(
        c.operate("tools.tasks", &a, &json!({})).await.unwrap()["result"]["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store.close().await;
    drop(c);
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let c = Core::new(store.clone());
    observe(&c, &a, &id, "recover", json!({"kind":"job.recover"})).await;
    assert_eq!(get(&c, &a, &id).await["status"], "unknown");
    assert_eq!(
        observe(
            &c,
            &a,
            &id,
            "try-again",
            json!({"kind":"job.claim","claim_id":"different"})
        )
        .await["execution_permit"],
        false
    );
    let changed = mint(
        &c,
        &a,
        "job.intent",
        None,
        Some(LaunchIdentity {
            call_id: "call-before-dispatch".into(),
            native_name: "native_tool".into(),
            arguments_sha256: storage::digest(&json!({"task":"changed"})).unwrap(),
        }),
    )
    .await;
    assert!(c.operate("harness.observe",&a,&json!({"scope_id":"session","grant_id":changed,"event_id":"changed-call","observation":{"kind":"job.intent","label":"changed"}})).await.is_err());
    store.close().await;
}
async fn mint(
    c: &Core,
    a: &OperationContext,
    name: &str,
    id: Option<&str>,
    launch: Option<LaunchIdentity>,
) -> String {
    c.issue_harness_grant(
        a,
        HarnessGrant {
            scope_id: "session".into(),
            chain_id: a.chain_id.clone(),
            turn_id: "trusted-turn".into(),
            human_message_id: None,
            role: if name.ends_with(".intent") || name.ends_with(".register") || name == "job.claim"
            {
                HarnessRole::Worker
            } else {
                HarnessRole::Executor
            },
            human_intent: HumanIntent::None,
            allowed_calls: vec![name.into()],
            object_id: id.map(str::to_owned),
            goal_round: None,
            max_goal_rounds: None,
            wait_timeout_ms: None,
            launch,
        },
    )
    .await
    .unwrap()["grant_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn get(c: &Core, a: &OperationContext, id: &str) -> Value {
    c.operate(
        "harness.get",
        a,
        &json!({"scope_id":"session","object_id":id}),
    )
    .await
    .unwrap()["result"]
        .clone()
}
async fn intent(c: &Core, a: &OperationContext, event: &str, call: &str, args: Value) -> Value {
    let grant = mint(
        c,
        a,
        "job.intent",
        None,
        Some(LaunchIdentity {
            call_id: call.into(),
            native_name: "native_tool".into(),
            arguments_sha256: storage::digest(&args).unwrap(),
        }),
    )
    .await;
    c.operate(
        "harness.observe",
        a,
        &json!({"scope_id":"session","grant_id":grant,"event_id":event,
        "observation":{"kind":"job.intent","label":"one real task","started_at_ms":10}}),
    )
    .await
    .unwrap()["result"]
        .clone()
}
async fn request(
    c: &Core,
    a: &OperationContext,
    id: &str,
    event: &str,
    observation: Value,
) -> Value {
    let current = get(c, a, id).await;
    let grant = mint(c, a, observation["kind"].as_str().unwrap(), Some(id), None).await;
    json!({"scope_id":"session","object_id":id,"expected_revision":current["revision"],"grant_id":grant,"event_id":event,"observation":observation})
}
async fn observe(
    c: &Core,
    a: &OperationContext,
    id: &str,
    event: &str,
    observation: Value,
) -> Value {
    c.operate(
        "harness.observe",
        a,
        &request(c, a, id, event, observation).await,
    )
    .await
    .unwrap()["result"]
        .clone()
}

#[tokio::test]
async fn one_logical_call_survives_intent_and_dispatch_crashes_without_duplicate_permits() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("owned-launch.db");
    let a = auth();
    let store = Store::create(&path).await.unwrap();
    let c = Core::new(store.clone());
    let first = intent(&c, &a, "intent", "call-one", json!({"task":"one"})).await;
    let id = first["object_id"].as_str().unwrap().to_owned();
    assert_eq!(first["execution_permit"], false);
    store.close().await;
    drop(c);
    drop(store); // crash after durable intent, before claim
    let store = Store::open(&path).await.unwrap();
    let c = Core::new(store.clone());
    let recovered = intent(&c, &a, "intent-retry", "call-one", json!({"task":"one"})).await;
    assert_eq!(recovered["object_id"], id);
    assert_eq!(recovered["task_id"], first["task_id"]);
    let claim = request(
        &c,
        &a,
        &id,
        "claim",
        json!({"kind":"job.claim","claim_id":"host-claim"}),
    )
    .await;
    let permitted = c.operate("harness.observe", &a, &claim).await.unwrap()["result"].clone();
    assert_eq!(permitted["execution_permit"], true);
    let mut starts = 0;
    if permitted["execution_permit"] == true {
        starts += 1;
    } // the only synthetic executor invocation
    assert_eq!(
        c.operate("harness.observe", &a, &claim).await.unwrap()["result"]["execution_permit"],
        false
    );
    store.close().await;
    drop(c);
    drop(store); // executor ran; crash before handle attachment
    let store = Store::open(&path).await.unwrap();
    let c = Core::new(store.clone());
    assert_eq!(
        intent(&c, &a, "intent-retry2", "call-one", json!({"task":"one"})).await["execution_permit"],
        false
    );
    observe(&c, &a, &id, "recover", json!({"kind":"job.recover"})).await;
    assert_eq!(get(&c, &a, &id).await["status"], "unknown");
    let retry = observe(
        &c,
        &a,
        &id,
        "claim-again",
        json!({"kind":"job.claim","claim_id":"another-claim"}),
    )
    .await;
    if retry["execution_permit"] == true {
        starts += 1;
    }
    assert_eq!(starts, 1);
    observe(&c,&a,&id,"attach",json!({"kind":"job.attach","claim_id":"host-claim","external_id":"native-original-id","executor":{"kind":"fixture","id":"original-handle"}})).await;
    let observed = mint(&c, &a, "job.register", None, None).await;
    let again=c.operate("harness.observe",&a,&json!({"scope_id":"session","grant_id":observed,"event_id":"native-registered-after-attach",
        "observation":{"kind":"job.register","external_id":"native-original-id","label":"native observer","started_at_ms":10,"executor":{"kind":"fixture","id":"original-handle"}}})).await.unwrap();
    assert_eq!(again["result"]["object_id"], id);
    let list = c
        .operate(
            "harness.call",
            &a,
            &json!({"scope_id":"session","name":"job_list","arguments":{}}),
        )
        .await
        .unwrap();
    assert_eq!(list["result"]["value"][0]["id"], "native-original-id");
    assert_eq!(list["result"]["value"].as_array().unwrap().len(), 1);
    observe(
        &c,
        &a,
        &id,
        "remove",
        json!({"kind":"job.remove","reason":"foreground"}),
    )
    .await;
    assert_eq!(get(&c, &a, &id).await["status"], "running");
    assert!(
        c.operate(
            "harness.call",
            &a,
            &json!({"scope_id":"session","name":"job_list","arguments":{}})
        )
        .await
        .unwrap()["result"]["value"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    observe(&c,&a,&id,"settle",json!({"kind":"job.settle","status":"completed","result":"actual output","finished_at_ms":20})).await;
    assert_eq!(get(&c, &a, &id).await["dispatch_state"], "settled");
    store.close().await;
}
