use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    harness_lifecycle::{HarnessGrant, HarnessRole, HumanIntent},
    storage::Store,
};
#[tokio::test]
async fn native_activity_result_and_channel_projection_are_durable_and_immutable() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let registered = register(&c, "job.register", "native-channel-job").await;
    let id = registered["object_id"].as_str().unwrap();
    let native = json!({"isError":false,"value":{"kind":"background","jobId":"native-channel-job"},"content":[{"type":"text","text":"original native output"}],"meta":{"cwd":"fixture"}});
    activity_observe(&c,id,"native-tool-result",json!({"kind":"job.tool_result","result":native})).await;
    activity_observe(&c,id,"native-view",json!({"kind":"job.native_view","view":{"id":"native-channel-job","output":{"total":14,"earliest":0},"progress":"native progress"}})).await;
    for (i, channel, text, at) in [(0,"stdout","out",0),(1,"log","hidden",3),(2,"stderr","err",9)] {
        activity_observe(&c,id,&format!("chunk-{i}"),json!({"kind":"job.output","text":text,"native_chunk":{"at":at,"text":text,"channel":channel}})).await;
    }
    let result = call_job(&c,id,"job_output","collect-channels",json!({"job_id":"native-channel-job"})).await;
    assert_eq!(result["value"]["text"], "out\n[stderr]\nerr");
    let state = get(&c,id).await;
    assert_eq!(state["native_tool_result"],native);
    assert_eq!(state["native_view"]["progress"],"native progress");
    assert_eq!(result["value"]["job"]["detail"],"native progress");
    let g = mint(&c,HarnessRole::Executor,false,"job.tool_result",Some(id),None,None).await;
    let changed = json!({"scope_id":"session","object_id":id,"expected_revision":state["revision"],"grant_id":g,"event_id":"changed-native-result","observation":{"kind":"job.tool_result","result":{"isError":false,"value":"forged different result"}}});
    assert!(c.operate("harness.observe",&auth(),&changed).await.is_err());
    assert_eq!(get(&c,id).await["native_tool_result"],native);
    store.close().await;
}
#[tokio::test]
async fn native_clear_is_human_only_cas_tombstone_and_allows_a_fresh_goal() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let made = create(&c, 5).await;
    let id = made["object_id"].as_str().unwrap();
    observe(&c, id, 1, "goal.round.start", None).await;
    observe(&c, id, 1, "goal.round.finish", None).await;
    let before = get(&c, id).await;
    let denied = mint(&c, HarnessRole::Worker, false, "clear_goal", Some(id), None, None).await;
    let payload = json!({"scope_id":"session","name":"clear_goal","arguments":{"goal_id":id,"revision":1},"grant_id":denied,"event_id":"deny-clear"});
    assert!(c.operate("harness.call", &auth(), &payload).await.is_err());
    assert_eq!(get(&c, id).await["revision"], before["revision"]);
    let grant = mint(&c, HarnessRole::Worker, true, "clear_goal", Some(id), None, None).await;
    let mut payload = json!({"scope_id":"session","name":"clear_goal","arguments":{"goal_id":id,"revision":2},"grant_id":grant,"event_id":"human-clear"});
    assert!(c.operate("harness.call", &auth(), &payload).await.is_err());
    payload["arguments"]["revision"] = json!(1);
    let cleared = c.operate("harness.call", &auth(), &payload).await.unwrap();
    assert_eq!(cleared["result"]["value"]["cleared"], json!({"id":id,"revision":2}));
    assert_eq!(c.operate("harness.call", &auth(), &payload).await.unwrap()["result"]["replayed"], true);
    let tombstone = get(&c, id).await;
    assert_eq!(tombstone["cleared"], true);
    assert_eq!(tombstone["revision"], before["revision"].as_u64().unwrap() + 1);
    assert_eq!(tombstone["round_receipts"], before["round_receipts"]);
    let read = c.operate("harness.call", &auth(), &json!({"scope_id":"session","name":"get_goal","arguments":{}})).await.unwrap();
    assert_eq!(read["result"]["value"], json!({"goal":null}));
    assert!(update(&c, id, "resume", true, 0).await.is_err());
    let grant = mint(&c, HarnessRole::Worker, true, "create_goal", None, None, None).await;
    let fresh = c.operate("harness.call", &auth(), &json!({"scope_id":"session","name":"create_goal","arguments":{"objective":"fresh task"},"grant_id":grant,"event_id":"create-fresh"})).await.unwrap();
    assert_ne!(fresh["result"]["object_id"], made["object_id"]);
    store.close().await;
}
#[tokio::test]
async fn durable_job_cursor_reopen_and_wait_release_the_write_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-lifecycle-fixture.db");
    let store = Store::create(&path).await.unwrap();
    let c = Core::new(store.clone());
    let registered = register(&c, "job.register", "persistent-job").await;
    let id = registered["object_id"].as_str().unwrap().to_owned();
    activity_observe(
        &c,
        &id,
        "persist-output",
        json!({"kind":"job.output","text":"persisted"}),
    )
    .await;
    let read = job_request(
        &c,
        &id,
        "job_output",
        "persist-read",
        json!({"job_id":"persistent-job"}),
    )
    .await;
    assert_eq!(
        c.operate("harness.call", &auth(), &read).await.unwrap()["result"]["value"]["text"],
        "persisted"
    );
    store.close().await;
    drop(c);
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let c = Core::new(store.clone());
    assert_eq!(get(&c, &id).await["output_cursor"], 1);
    let next = job_request(
        &c,
        &id,
        "job_output",
        "after-reopen",
        json!({"job_id":"persistent-job","wait":true,"timeout_ms":1000}),
    )
    .await;
    let principal = auth();
    let (output, _) = tokio::join!(c.operate("harness.call", &principal, &next), async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        activity_observe(
            &c,
            &id,
            "after-reopen-settle",
            json!({"kind":"job.settle","status":"completed","result":"final","finished_at_ms":400}),
        )
        .await;
    });
    let output = output.unwrap();
    assert_eq!(output["result"]["value"]["text"], "final");
    assert_eq!(output["result"]["value"]["job"]["status"], "completed");
    let running = register(&c, "job.register", "still-running").await;
    let running_id = running["object_id"].as_str().unwrap();
    let request = job_request(
        &c,
        running_id,
        "job_output",
        "timeout-read",
        json!({"job_id":"still-running","wait":true,"timeout_ms":3}),
    )
    .await;
    let value = c.operate("harness.call", &auth(), &request).await.unwrap();
    assert_eq!(value["result"]["value"]["job"]["status"], "running");
    assert_eq!(get(&c, running_id).await["status"], "running");
    store.close().await;
}
#[tokio::test]
async fn native_goal_revision_survives_observation_cas_updates_and_original_round_source_fences() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let created = create(&c, 5).await;
    let id = created["object_id"].as_str().unwrap();
    let queued_revision = created["value"]["goal"]["revision"].clone();
    observe(&c, id, 1, "goal.round.start", None).await;
    let state = get(&c, id).await;
    assert_eq!(state["revision"], 2);
    assert_eq!(state["native_revision"], queued_revision);
    observe(&c, id, 1, "goal.round.finish", None).await;
    let state = get(&c, id).await;
    let grant = mint(
        &c,
        HarnessRole::Executor,
        false,
        "goal.disarm",
        Some(id),
        None,
        None,
    )
    .await;
    c.operate("harness.observe",&auth(),&json!({"scope_id":"session","object_id":id,"expected_revision":state["revision"],"grant_id":grant,"event_id":"driver-disarm","observation":{"kind":"goal.disarm"}})).await.unwrap();
    let goal = c
        .operate(
            "harness.call",
            &auth(),
            &json!({"scope_id":"session","name":"get_goal","arguments":{}}),
        )
        .await
        .unwrap();
    assert_eq!(goal["result"]["value"]["goal"]["revision"], queued_revision);
    assert_eq!(get(&c, id).await["revision"], 4);
    let resumed = update(&c, id, "resume", true, 1).await.unwrap();
    assert_eq!(resumed["result"]["value"]["goal"]["revision"], 2);
    assert_eq!(resumed["result"]["revision"], 5);
    observe(&c, id, 2, "goal.round.start", None).await;
    assert_eq!(get(&c, id).await["native_revision"], 2);
    assert_eq!(get(&c, id).await["revision"], 6);
    store.close().await;
}
fn auth() -> OperationContext {
    OperationContext {
        actor: "fixture-host".into(),
        allow_write: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn interrupted_goal_round_reopens_disarmed_then_only_human_resume_can_start_next_round() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("interrupted-goal.db");
    let store = Store::create(&path).await.unwrap();
    let c = Core::new(store.clone());
    let created = create(&c, 5).await;
    let id = created["object_id"].as_str().unwrap().to_owned();
    observe(&c, &id, 1, "goal.round.start", None).await;
    store.close().await;
    drop(c);
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let c = Core::new(store.clone());
    assert!(update(&c, &id, "resume", true, 1).await.is_err());
    let state = get(&c, &id).await;
    let grant = mint(
        &c,
        HarnessRole::Executor,
        false,
        "goal.round.interrupted",
        Some(&id),
        Some(1),
        None,
    )
    .await;
    let payload = json!({"scope_id":"session","object_id":id,"expected_revision":state["revision"],"grant_id":grant,"event_id":"interrupted-round-1",
        "observation":{"kind":"goal.round.interrupted","reason":"host restarted without an attested round outcome"}});
    let interrupted = c
        .operate("harness.observe", &auth(), &payload)
        .await
        .unwrap();
    assert_eq!(interrupted["result"]["value"]["activation"], "disarmed");
    assert_eq!(
        c.operate("harness.observe", &auth(), &payload)
            .await
            .unwrap()["result"]["replayed"],
        true
    );
    let state = get(&c, &id).await;
    assert_eq!(state["roundsFinished"], 1);
    assert_eq!(state["blockedCount"], 0);
    assert_eq!(state["interrupted_rounds"].as_array().unwrap().len(), 1);
    assert!(update(&c, &id, "resume", false, 1).await.is_err());
    update(&c, &id, "resume", true, 1).await.unwrap();
    observe(&c, &id, 2, "goal.round.start", None).await;
    assert_eq!(get(&c, &id).await["roundsStarted"], 2);
    store.close().await;
}

#[tokio::test]
async fn absent_harness_round_cap_requires_no_extra_configuration() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let grant = mint(
        &c,
        HarnessRole::Worker,
        true,
        "create_goal",
        None,
        None,
        None,
    )
    .await;
    let done=c.operate("harness.call",&auth(),&json!({"scope_id":"session","name":"create_goal","arguments":{"objective":"ordinary uncapped harness goal"},"grant_id":grant,"event_id":"uncapped"})).await.unwrap();
    assert!(
        done["result"]["value"]["goal"]
            .get("maxGoalRounds")
            .is_none()
    );
    let id = done["result"]["object_id"].as_str().unwrap();
    observe(&c, id, 1, "goal.round.start", None).await;
    observe(&c, id, 1, "goal.round.finish", None).await;
    assert_eq!(get(&c, id).await["activation"], "armed");
    store.close().await;
}
async fn mint(
    c: &Core,
    role: HarnessRole,
    human: bool,
    name: &str,
    id: Option<&str>,
    round: Option<u64>,
    cap: Option<u64>,
) -> String {
    c.issue_harness_grant(
        &auth(),
        HarnessGrant {
            scope_id: "session".into(),
            chain_id: None,
            turn_id: "trusted-turn".into(),
            human_message_id: if human {
                Some("human-message".into())
            } else {
                None
            },
            role,
            human_intent: if human {
                HumanIntent::DirectHuman
            } else {
                HumanIntent::None
            },
            allowed_calls: vec![name.into()],
            object_id: id.map(str::to_owned),
            goal_round: round,
            max_goal_rounds: cap,
            wait_timeout_ms: None,
            launch: None,
        },
    )
    .await
    .unwrap()["grant_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn get(c: &Core, id: &str) -> Value {
    c.operate(
        "harness.get",
        &auth(),
        &json!({"scope_id":"session","object_id":id}),
    )
    .await
    .unwrap()["result"]
        .clone()
}
async fn create(c: &Core, cap: u64) -> Value {
    let g = mint(
        c,
        HarnessRole::Worker,
        true,
        "create_goal",
        None,
        None,
        Some(cap),
    )
    .await;
    c.operate("harness.call",&auth(),&json!({"scope_id":"session","name":"create_goal","arguments":{"objective":"finish actual task","max_goal_rounds":cap},"grant_id":g,"event_id":"create"})).await.unwrap()["result"].clone()
}
async fn register(c: &Core, name: &str, external: &str) -> Value {
    let g = mint(c, HarnessRole::Worker, false, name, None, None, None).await;
    c.operate("harness.observe",&auth(),&json!({"scope_id":"session","grant_id":g,"event_id":format!("register-{external}"),
        "observation":{"kind":name,"external_id":external,"job_kind":"fixture","label":"fixture child","started_at_ms":123,
            "output_mode":"stream","executor":{"kind":"local-fixture","id":format!("executor-{external}")}}})).await.unwrap()["result"].clone()
}
async fn activity_observe(c: &Core, id: &str, event: &str, o: Value) -> Value {
    let state = get(c, id).await;
    let g = mint(
        c,
        HarnessRole::Executor,
        false,
        o["kind"].as_str().unwrap(),
        Some(id),
        None,
        None,
    )
    .await;
    c.operate("harness.observe",&auth(),&json!({"scope_id":"session","object_id":id,"expected_revision":state["revision"],"grant_id":g,"event_id":event,"observation":o})).await.unwrap()["result"].clone()
}
async fn job_request(c: &Core, id: &str, name: &str, event: &str, args: Value) -> Value {
    let grant = mint(c, HarnessRole::Worker, false, name, Some(id), None, None).await;
    json!({"scope_id":"session","name":name,"arguments":args,"grant_id":grant,"event_id":event})
}
async fn call_job(c: &Core, id: &str, name: &str, event: &str, args: Value) -> Value {
    c.operate(
        "harness.call",
        &auth(),
        &job_request(c, id, name, event, args).await,
    )
    .await
    .unwrap()["result"]
        .clone()
}

#[tokio::test]
async fn jobs_cursors_cancellation_observations_and_workflow_peer_mapping_are_sic_state() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let job = register(&c, "job.register", "native-job").await;
    let id = job["object_id"].as_str().unwrap();
    activity_observe(&c, id, "out1", json!({"kind":"job.output","text":"chunk1"})).await;
    assert_eq!(
        call_job(
            &c,
            id,
            "job_output",
            "read1",
            json!({"job_id":"native-job"})
        )
        .await["value"]["text"],
        "chunk1"
    );
    assert_eq!(
        call_job(
            &c,
            id,
            "job_output",
            "read2",
            json!({"job_id":"native-job"})
        )
        .await["value"]["text"],
        ""
    );
    activity_observe(&c, id, "out2", json!({"kind":"job.output","text":"chunk2"})).await;
    let kill = job_request(
        &c,
        id,
        "job_kill",
        "kill",
        json!({"job_id":"native-job","reason":"no longer needed"}),
    )
    .await;
    let done = c.operate("harness.call", &auth(), &kill).await.unwrap()["result"].clone();
    assert_eq!(done["value"]["outcome"], "cancellation-requested");
    assert_eq!(done["value"]["job"]["status"], "stopping");
    assert_eq!(
        done["executor_request"]["executor"]["id"],
        "executor-native-job"
    );
    assert_eq!(
        c.operate("harness.call", &auth(), &kill).await.unwrap()["result"]["executor_request"],
        done["executor_request"]
    );
    activity_observe(&c,id,"settle",json!({"kind":"job.settle","status":"killed","result":"cleanup complete","finished_at_ms":321})).await;
    let output = call_job(
        &c,
        id,
        "job_output",
        "read3",
        json!({"job_id":"native-job"}),
    )
    .await;
    assert_eq!(output["value"]["text"], "chunk2\ncleanup complete");
    assert!(output["collection"]["text_digest"].is_string());
    assert_eq!(get(&c, id).await["adopted"], false);
    assert_eq!(
        call_job(
            &c,
            id,
            "job_output",
            "read4",
            json!({"job_id":"native-job"})
        )
        .await["value"]["text"],
        ""
    );
    register(&c, "workflow.register", "native-workflow").await;
    register(&c, "peer.register", "native-peer").await;
    let listing = c
        .operate(
            "harness.call",
            &auth(),
            &json!({"scope_id":"session","name":"job_list","arguments":{}}),
        )
        .await
        .unwrap();
    assert_eq!(listing["result"]["value"].as_array().unwrap().len(), 3);
    assert_eq!(listing["result"]["bindings"].as_array().unwrap().len(), 3);
    store.close().await;
}
async fn observe(c: &Core, id: &str, round: u64, kind: &str, condition: Option<&str>) -> Value {
    let s = get(c, id).await;
    let g = mint(
        c,
        HarnessRole::Executor,
        false,
        kind,
        Some(id),
        Some(round),
        None,
    )
    .await;
    let p = json!({"scope_id":"session","object_id":id,"expected_revision":s["revision"],"grant_id":g,
        "event_id":format!("{kind}-{round}"),"observation":{"kind":kind,"outcome":if condition.is_some(){"blocked"}else{"progress"},"condition":condition}});
    let done = c.operate("harness.observe", &auth(), &p).await.unwrap()["result"].clone();
    assert_eq!(
        c.operate("harness.observe", &auth(), &p).await.unwrap()["result"]["replayed"],
        true
    );
    done
}
async fn update(
    c: &Core,
    id: &str,
    action: &str,
    human: bool,
    round: u64,
) -> sic_core_rust::error::Result<Value> {
    let s = get(c, id).await;
    let g = mint(
        c,
        HarnessRole::Worker,
        human,
        "update_goal",
        Some(id),
        Some(round),
        None,
    )
    .await;
    c.operate("harness.call",&auth(),&json!({"scope_id":"session","name":"update_goal","grant_id":g,
        "event_id":format!("{action}-{}",s["revision"]),"arguments":{"goal_id":id,"revision":s.get("native_revision").unwrap_or(&s["revision"]),"action":action,"blocked_reason":"missing dependency","human":true}})).await
}

#[tokio::test]
async fn exact_round_receipts_three_condition_threshold_resume_reset_and_round_cap_are_enforced() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    let created = create(&c, 5).await;
    let id = created["object_id"].as_str().unwrap();
    for round in 1..=3 {
        observe(&c, id, round, "goal.round.start", None).await;
        observe(
            &c,
            id,
            round,
            "goal.round.finish",
            Some("missing dependency"),
        )
        .await;
        if round < 3 {
            assert!(update(&c, id, "blocked", false, round).await.is_err());
        }
    }
    let blocked = update(&c, id, "blocked", false, 3).await.unwrap();
    assert_eq!(blocked["result"]["value"]["goal"]["phase"], "blocked");
    assert_eq!(
        get(&c, id).await["round_receipts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(update(&c, id, "resume", false, 3).await.is_err());
    update(&c, id, "resume", true, 3).await.unwrap();
    assert_eq!(get(&c, id).await["blockedCount"], 0);
    observe(&c, id, 4, "goal.round.start", None).await;
    observe(&c, id, 4, "goal.round.finish", Some("missing dependency")).await;
    assert!(update(&c, id, "blocked", false, 4).await.is_err());
    observe(&c, id, 5, "goal.round.start", None).await;
    observe(&c, id, 5, "goal.round.finish", None).await;
    assert_eq!(get(&c, id).await["activation"], "disarmed");
    let s = get(&c, id).await;
    let g = mint(
        &c,
        HarnessRole::Executor,
        false,
        "goal.round.start",
        Some(id),
        Some(6),
        None,
    )
    .await;
    assert!(c.operate("harness.observe",&auth(),&json!({"scope_id":"session","object_id":id,"expected_revision":s["revision"],"grant_id":g,"event_id":"over-cap","observation":{"kind":"goal.round.start"}})).await.is_err());
    update(&c, id, "complete", false, 5).await.unwrap();
    assert_eq!(get(&c, id).await["phase"], "complete");
    store.close().await;
}

#[tokio::test]
async fn human_authority_cannot_be_self_declared_in_model_arguments_or_grant_rpc() {
    let store = Store::memory().await.unwrap();
    let c = Core::new(store.clone());
    assert!(
        c.operate("harness.grant", &auth(), &json!({"human":true}))
            .await
            .is_err()
    );
    let g = mint(
        &c,
        HarnessRole::Worker,
        false,
        "create_goal",
        None,
        None,
        Some(5),
    )
    .await;
    assert!(c.operate("harness.call",&auth(),&json!({"scope_id":"session","name":"create_goal","arguments":{"objective":"forged","human":true,"role":"human"},"grant_id":g,"event_id":"forged"})).await.is_err());
    assert!(
        c.issue_harness_grant(
            &auth(),
            HarnessGrant {
                scope_id: "session".into(),
                role: HarnessRole::Editor,
                chain_id: None,
                turn_id: "trusted-turn".into(),
                human_message_id: Some("human-message".into()),
                human_intent: HumanIntent::DirectHuman,
                allowed_calls: vec!["create_goal".into()],
                object_id: None,
                goal_round: None,
                max_goal_rounds: Some(5),
                wait_timeout_ms: None,
                launch: None
            }
        )
        .await
        .is_err()
    );
    let goal = create(&c, 5).await;
    let id = goal["object_id"].as_str().unwrap();
    assert_eq!(goal["value"]["goal"]["revision"], 1);
    assert_eq!(goal["value"]["activation"], "armed");
    let other = OperationContext {
        actor: "another-host".into(),
        ..auth()
    };
    assert!(
        c.operate(
            "harness.get",
            &other,
            &json!({"scope_id":"session","object_id":id})
        )
        .await
        .is_err()
    );
    store.close().await;
}

#[tokio::test]
async fn authenticated_host_turn_http_path_mints_grant_and_native_goal_rpc_uses_it() {
    use sic_core_rust::server::{self, HostAuth};
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = server::router(
        core,
        HostAuth::new("fixture-token", "fixture-host", true, false, false).unwrap(),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let metadata = json!({"scope_id":"http-session","turn_id":"trusted-turn-1","human_message_id":"root-user-1",
        "role":"worker","human_intent":"direct_human","allowed_calls":["create_goal"],"object_id":null,"goal_round":null,"max_goal_rounds":3});
    assert_eq!(
        client
            .post(format!("{base}/api/v2/harness/turn"))
            .json(&metadata)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    let grant: Value = client
        .post(format!("{base}/api/v2/harness/turn"))
        .bearer_auth("fixture-token")
        .json(&metadata)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let response:Value=client.post(format!("{base}/api/v2/operations")).bearer_auth("fixture-token").json(&json!({
        "operation":"harness.call","context":{"actor":"fixture-host","allow_write":true},
        "payload":{"scope_id":"http-session","name":"create_goal","grant_id":grant["grant_id"],"event_id":"native-call-1","arguments":{"objective":"actual persisted HTTP goal"}}
    })).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(
        response["result"]["value"]["goal"]["objective"],
        "actual persisted HTTP goal"
    );
    assert_eq!(response["result"]["value"]["goal"]["maxGoalRounds"], 3);
    assert!(
        !sic_core_rust::core::Core::capabilities()["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["operation"] == "harness.grant")
    );
    server.abort();
    store.close().await;
}
