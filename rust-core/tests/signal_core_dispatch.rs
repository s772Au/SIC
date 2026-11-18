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
        allow_revoke: false,
    }
}

async fn call(core: &Core, context: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, context, &payload)
        .await
        .unwrap_or_else(|error| panic!("{operation} failed: {error:?}"))["result"]
        .clone()
}

async fn try_call(
    core: &Core,
    context: &OperationContext,
    operation: &str,
    payload: Value,
) -> Result<Value, String> {
    core.operate(operation, context, &payload)
        .await
        .map(|response| response["result"].clone())
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

async fn agent(core: &Core, actor: &str) -> OperationContext {
    let created = call(core, &context(actor, None), "chain.create", json!({})).await;
    let chain = created["chain_id"].as_str().unwrap().to_owned();
    context(actor, Some(&chain))
}

#[tokio::test]
async fn spawn_forks_and_status_run_through_the_core_dispatch() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let other = agent(&core, "bob").await;
    let me_id = me.chain_id.clone().unwrap();
    let other_id = other.chain_id.clone().unwrap();

    let status = call(&core, &me, "signal.status", json!({})).await;
    assert_eq!(status["route_id"], me_id.as_str());
    assert_eq!(status["state"]["state"], "running");

    let spawned = call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"child-1"}),
    )
    .await;
    assert_eq!(spawned["origin"]["kind"], "spawned");
    assert_eq!(spawned["depth"], 0);

    let forked = call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"child-2","origin_route_id":me_id,
              "prefix_cb_ids":["cb_1"],"origin_cut_id":"cut_1"}),
    )
    .await;
    assert_eq!(forked["origin"]["kind"], "forked");
    assert_eq!(forked["origin"]["self_fork"], true);
    assert_eq!(forked["depth"], 1);
    assert_eq!(forked["parents"], json!([me_id]));

    let joint = call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"child-3","co_parents":[other_id]}),
    )
    .await;
    assert_eq!(joint["parents"], json!([me_id, other_id]));

    let bob_view = call(&core, &other, "signal.status", json!({"target_route_id":"child-3"})).await;
    assert_eq!(bob_view["parents"], json!([me_id, other_id]));
}

#[tokio::test]
async fn pause_and_stop_mirror_onto_the_agent_run_state_guards() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();

    let paused = call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":me_id,"mode":"now"}),
    )
    .await;
    assert_eq!(paused["state"]["state"], "paused");

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "paused");

    let prepare = try_call(
        &core,
        &me,
        "edit.prepare",
        json!({"source_id":"missing","editor":"editor-1"}),
    )
    .await;
    assert!(prepare.is_err(), "a paused Agent must refuse a new edit job");

    call(
        &core,
        &me,
        "signal.source.register",
        json!({"source_id":"reply-1","kind":"child_reply"}),
    )
    .await;
    let stopped = call(
        &core,
        &me,
        "signal.stop",
        json!({"target_route_id":me_id,"mode":"now","wake_sources":["reply-1"]}),
    )
    .await;
    assert_eq!(stopped["terminal"], false);

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "stopped");
}

#[tokio::test]
async fn queued_arrivals_survive_a_stop_and_wake_on_an_admitted_source() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();

    for (id, kind) in [("reply-1", "child_reply"), ("msg-1", "agent_message")] {
        call(
            &core,
            &me,
            "signal.source.register",
            json!({"source_id":id,"kind":kind}),
        )
        .await;
    }
    call(
        &core,
        &me,
        "signal.stop",
        json!({"target_route_id":me_id,"mode":"now","wake_sources":["reply-1"]}),
    )
    .await;

    let queued = call(
        &core,
        &me,
        "signal.arrive",
        json!({"source_id":"msg-1","target_route_id":me_id,"payload":{"text":"later"}}),
    )
    .await;
    assert_eq!(queued["admitted"], false);
    assert_eq!(queued["queued"], true);
    assert_eq!(queued["resumed"], false);
    assert!(queued["received_at"].as_u64().is_some());

    let woken = call(
        &core,
        &me,
        "signal.arrive",
        json!({"source_id":"reply-1","target_route_id":me_id,"payload":{}}),
    )
    .await;
    assert_eq!(woken["resumed"], true);

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "running");

    let drained = call(
        &core,
        &me,
        "signal.drain",
        json!({"target_route_id":me_id,"limit":8}),
    )
    .await;
    let items = drained["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["source_id"], "reply-1");
    assert_eq!(drained["remaining_queue"], 1);
}

#[tokio::test]
async fn i_broadcast_carries_the_management_log_into_the_agent_region() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"kid","origin_route_id":me_id,"prefix_cb_ids":["cb_1"]}),
    )
    .await;
    call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":"kid","mode":"now"}),
    )
    .await;

    let broadcast = call(&core, &me, "signal.broadcast", json!({"log_limit":8})).await;
    let children = broadcast["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["route_id"], "kid");
    assert_eq!(children[0]["state"]["state"], "paused");
    assert!(children[0]["since"].as_u64().is_some());
    let log = broadcast["management_log"].as_array().unwrap();
    assert!(log.iter().all(|entry| entry["at"].as_u64().is_some()));
    assert!(log.iter().any(|entry| entry["action"] == "signal.pause"));
    assert!(log.iter().any(|entry| entry["action"] == "signal.spawn"));
}

#[tokio::test]
async fn arrivals_from_an_unrelated_agent_need_the_wake_right() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let other = agent(&core, "bob").await;
    let me_id = me.chain_id.clone().unwrap();
    let other_id = other.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.source.register",
        json!({"source_id":"reply-1","kind":"child_reply"}),
    )
    .await;
    call(
        &core,
        &me,
        "signal.source_policy",
        json!({"target_route_id":me_id,"kinds":["child_reply"]}),
    )
    .await;
    call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":me_id,"mode":"now"}),
    )
    .await;

    let denied = try_call(
        &core,
        &other,
        "signal.arrive",
        json!({"source_id":"reply-1","target_route_id":me_id,"payload":{}}),
    )
    .await;
    assert!(denied.is_err(), "bob holds no wake right over alice");

    call(
        &core,
        &me,
        "signal.admin_grant",
        json!({"target_route_id":me_id,"holder_route_id":other_id,"rights":["wake"]}),
    )
    .await;
    let allowed = call(
        &core,
        &other,
        "signal.arrive",
        json!({"source_id":"reply-1","target_route_id":me_id,"payload":{}}),
    )
    .await;
    assert_eq!(allowed["resumed"], true);
}

#[tokio::test]
async fn sleep_is_a_pause_and_not_a_terminal_stop() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();

    let slept = call(
        &core,
        &me,
        "signal.sleep",
        json!({"timeout_s":30,"reason":"waiting for a reply"}),
    )
    .await;
    assert_eq!(slept["state"]["state"], "paused");
    assert_eq!(slept["timeout_s"], 30);

    let status = call(&core, &me, "signal.status", json!({})).await;
    assert_eq!(status["state"]["state"], "paused");
    assert_ne!(status["state"]["state"], "stopped");

    let clock = slept["clock_source"].as_str().unwrap().to_owned();
    let woke = call(
        &core,
        &me,
        "signal.arrive",
        json!({"source_id":clock,"target_route_id":me_id,"payload":{}}),
    )
    .await;
    assert_eq!(woke["resumed"], true);
}

#[tokio::test]
async fn signal_state_survives_a_core_reopen() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"kid","origin_route_id":me_id,"prefix_cb_ids":["cb_1"]}),
    )
    .await;
    call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":"kid","mode":"now"}),
    )
    .await;

    let reopened = Core::new(store);
    let status = call(&reopened, &me, "signal.status", json!({"target_route_id":"kid"})).await;
    assert_eq!(status["state"]["state"], "paused");
    assert_eq!(status["depth"], 1);
}

#[tokio::test]
async fn depth_and_children_limits_reject_through_the_core() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"root","limits":{"max_child_depth":1,"max_owned_children":1}}),
    )
    .await;
    call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"l1","origin_route_id":"root","prefix_cb_ids":["cb_1"]}),
    )
    .await;
    let too_deep = try_call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"l2","origin_route_id":"l1","prefix_cb_ids":["cb_1"]}),
    )
    .await;
    assert!(too_deep.unwrap_err().contains("signal_depth_limit"));
    let too_many = try_call(
        &core,
        &me,
        "signal.spawn",
        json!({"child_route_id":"l1b","origin_route_id":"root","prefix_cb_ids":["cb_1"]}),
    )
    .await;
    assert!(too_many.unwrap_err().contains("signal_children_limit"));
    let _ = me_id;
}
