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

async fn agent(core: &Core, actor: &str) -> OperationContext {
    let created = call(core, &context(actor, None), "chain.create", json!({})).await;
    let chain = created["chain_id"].as_str().unwrap().to_owned();
    context(actor, Some(&chain))
}

#[tokio::test]
async fn pause_after_output_mirrors_a_pending_state_that_still_accepts_input() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();

    let paused = call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":me_id,"mode":"after_output"}),
    )
    .await;
    assert_eq!(paused["state"]["state"], "pause_pending");

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "pause_pending");

    // A pending pause must not disable the completion that is already
    // running: only a settled Paused state gates the next input.
    let status = call(&core, &me, "signal.status", json!({})).await;
    assert_eq!(status["state"]["state"], "pause_pending");
}

#[tokio::test]
async fn stop_after_output_is_pending_until_settled() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
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
        json!({"target_route_id":me_id,"mode":"after_output","wake_sources":["reply-1"]}),
    )
    .await;
    assert_eq!(stopped["state"]["state"], "stop_pending");
    assert_eq!(stopped["terminal"], false);

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "stop_pending");
}

#[tokio::test]
async fn signal_tools_are_advertised_in_the_common_registry() {
    let names = Core::common_tool_names();
    for expected in [
        "sic_signal_status",
        "sic_signal_broadcast",
        "sic_signal_pause",
        "sic_signal_sleep",
        "sic_signal_drain",
        "sic_signal_register",
        "sic_signal_arrive",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} must be in the shared tool registry"
        );
    }
    // The pre-existing Signal tools stay present; this change only adds.
    for expected in ["sic_signal_send", "sic_signal_wait"] {
        assert!(names.iter().any(|name| name == expected));
    }
}

#[tokio::test]
async fn status_exposes_held_rights_and_delegations() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let other = agent(&core, "bob").await;
    let me_id = me.chain_id.clone().unwrap();
    let other_id = other.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.admin_grant",
        json!({"target_route_id":me_id,"holder_route_id":other_id,"rights":["pause","source_policy"]}),
    )
    .await;
    let view = call(&core, &me, "signal.status", json!({})).await;
    let delegations = view["delegations"].as_array().unwrap();
    assert_eq!(delegations.len(), 1);
    assert_eq!(delegations[0]["holder_route_id"], other_id.as_str());
    let rights = delegations[0]["rights"].as_array().unwrap();
    assert_eq!(rights.len(), 2);
}

#[tokio::test]
async fn broadcast_reports_nodes_without_a_parent_link() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let broadcast = call(&core, &me, "signal.broadcast", json!({})).await;
    assert_eq!(broadcast["children"].as_array().unwrap().len(), 0);
    assert_eq!(broadcast["queue_depth"], 0);
    assert_eq!(broadcast["signal_state"]["state"], "running");
    assert_eq!(broadcast["control_epoch"], 0);
}

#[tokio::test]
async fn pending_states_accept_input_while_settled_states_do_not() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
    call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":me_id,"mode":"now"}),
    )
    .await;
    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "paused");
    // A settled pause refuses the next edit job through the existing
    // run_state guard, which Signal did not have to rewrite.
    let blocked = core
        .operate(
            "edit.prepare",
            &me,
            &json!({"source_id":"missing","editor":"editor-1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "conflict");
    assert!(
        blocked.message.contains("paused"),
        "the existing guard must report the Signal state: {}",
        blocked.message
    );
}

#[tokio::test]
async fn a_pending_pause_does_not_interrupt_the_attempt_that_is_in_flight() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();

    // Pause with after_output: the settled state must not be applied yet.
    let paused = call(
        &core,
        &me,
        "signal.pause",
        json!({"target_route_id":me_id,"mode":"after_output"}),
    )
    .await;
    assert_eq!(paused["state"]["state"], "pause_pending");

    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "pause_pending");

    // pause_pending is not a settled state, so the existing edit guards keep
    // this route available: the after-output Signal defers, it does not cut.
    let status = call(&core, &me, "signal.status", json!({})).await;
    assert_eq!(status["state"]["state"], "pause_pending");
    assert_eq!(status["state"]["state"], "pause_pending");
}
#[tokio::test]
async fn a_settled_stop_ends_the_queue_before_the_next_attempt() {
    let core = Core::new(Store::memory().await.unwrap());
    let me = agent(&core, "alice").await;
    let me_id = me.chain_id.clone().unwrap();
    let before = call(&core, &me, "chain.get", json!({})).await;
    let epoch_before = before["control_epoch"].as_u64().unwrap();

    let stopped = call(
        &core,
        &me,
        "signal.stop",
        json!({"target_route_id":me_id,"mode":"now"}),
    )
    .await;
    assert_eq!(stopped["terminal"], true);
    let chain = call(&core, &me, "chain.get", json!({})).await;
    assert_eq!(chain["run_state"], "stopped");
    // A settled Stop takes authority away from anything already in flight.
    assert!(
        chain["control_epoch"].as_u64().unwrap() > epoch_before,
        "a terminal Stop must bump the control epoch"
    );

    // The queue refuses further admission and further Signal control.
    let blocked = core
        .operate(
            "edit.prepare",
            &me,
            &json!({"source_id":"missing","editor":"editor-1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "conflict");
    assert!(blocked.message.contains("paused or stopped"), "{}", blocked.message);

    let again = core
        .operate(
            "signal.pause",
            &me,
            &json!({"target_route_id":me_id,"mode":"now"}),
        )
        .await
        .unwrap_err();
    assert_eq!(again.status, 409, "a terminal Agent cannot be paused again");
}
