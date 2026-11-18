use serde_json::json;
use sic_core_rust::ipc::Principal;
use sic_core_rust::signal::{
    ApplyMode, ControlRight, ControlState, SignalLimits, SignalState, SourceKind, SpawnSpec,
};
use std::collections::BTreeSet;

fn principal(route: &str) -> Principal {
    Principal {
        actor_id: format!("actor-{route}"),
        route_id: route.to_owned(),
    }
}

fn spawn(root: &mut SignalState, route: &str) {
    let spec = SpawnSpec {
        child_route_id: route.to_owned(),
        ..Default::default()
    };
    root.spawn(&principal(route), spec).expect("spawn");
}

fn fork(
    root: &mut SignalState,
    requester: &str,
    origin: &str,
    child: &str,
    cb_ids: Vec<&str>,
) {
    let spec = SpawnSpec {
        child_route_id: child.to_owned(),
        co_parents: Vec::new(),
        origin_route_id: Some(origin.to_owned()),
        prefix_cb_ids: cb_ids.into_iter().map(str::to_owned).collect(),
        origin_cut_id: None,
        limits: None,
        initial_policy: None,
    };
    root.spawn(&principal(requester), spec).expect("fork");
}

fn source(
    root: &mut SignalState,
    requester: &str,
    id: &str,
    kind: SourceKind,
) {
    root.register_source(&principal(requester), id, kind, None)
        .expect("register");
}

#[test]
fn empty_prefix_is_spawn_and_named_prefix_is_fork() {
    let mut root = SignalState::new();
    spawn(&mut root, "agent-a");
    assert!(root.node("agent-a").unwrap().origin.is_spawn());
    fork(&mut root, "agent-a", "agent-a", "child-a", vec!["cb_1", "cb_2"]);
    let child = root.node("child-a").unwrap();
    assert_eq!(child.parents, vec!["agent-a".to_owned()]);
    assert_eq!(child.depth, 1);
    match &child.origin {
        sic_core_rust::signal::ForkOrigin::Forked {
            origin_route_id,
            prefix_cb_ids,
            self_fork,
            ..
        } => {
            assert_eq!(origin_route_id, "agent-a");
            assert_eq!(prefix_cb_ids, &vec!["cb_1".to_owned(), "cb_2".to_owned()]);
            assert!(*self_fork);
        }
        other => panic!("expected a self fork, got {other:?}"),
    }
}

#[test]
fn fork_requires_a_prefix_and_spawn_rejects_one() {
    let mut root = SignalState::new();
    spawn(&mut root, "agent-a");
    let empty_prefix = SpawnSpec {
        child_route_id: "bad-fork".into(),
        origin_route_id: Some("agent-a".into()),
        ..Default::default()
    };
    assert!(root.spawn(&principal("agent-a"), empty_prefix).is_err());
    let prefix_without_origin = SpawnSpec {
        child_route_id: "bad-spawn".into(),
        prefix_cb_ids: vec!["cb_1".into()],
        ..Default::default()
    };
    assert!(
        root.spawn(&principal("agent-a"), prefix_without_origin)
            .is_err()
    );
}

#[test]
fn co_parents_share_management_rights_on_the_child() {
    let mut root = SignalState::new();
    spawn(&mut root, "agent-a");
    spawn(&mut root, "agent-b");
    let spec = SpawnSpec {
        child_route_id: "child-ab".into(),
        co_parents: vec!["agent-b".into()],
        ..Default::default()
    };
    root.spawn(&principal("agent-a"), spec).expect("co-parent spawn");
    let joint = root.node("child-ab").unwrap();
    assert_eq!(joint.parents, vec!["agent-a".to_owned(), "agent-b".to_owned()]);
    for parent in ["agent-a", "agent-b"] {
        let rights = root.held(&principal(parent), "child-ab");
        assert_eq!(rights, ControlRight::all());
    }
}

#[test]
fn depth_and_children_limits_are_enforced() {
    let mut root = SignalState::new();
    let tight = SignalLimits { max_child_depth: 1, max_owned_children: 1 };
    let spec = SpawnSpec {
        child_route_id: "root".into(),
        limits: Some(tight),
        ..Default::default()
    };
    root.spawn(&principal("root"), spec).expect("root spawn");
    fork(&mut root, "root", "root", "l1", vec!["cb_1"]);
    let too_deep = SpawnSpec {
        child_route_id: "l2".into(),
        origin_route_id: Some("l1".into()),
        prefix_cb_ids: vec!["cb_1".into()],
        ..Default::default()
    };
    let error = root
        .spawn(&principal("l1"), too_deep)
        .expect_err("depth limit");
    assert_eq!(error.code, "signal_depth_limit");
    let second_child = SpawnSpec {
        child_route_id: "l1b".into(),
        origin_route_id: Some("root".into()),
        prefix_cb_ids: vec!["cb_1".into()],
        ..Default::default()
    };
    let error = root
        .spawn(&principal("root"), second_child)
        .expect_err("children limit");
    assert_eq!(error.code, "signal_children_limit");
}

#[test]
fn pause_now_suspends_and_pause_after_output_waits() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    let me = principal("self");
    let out = root
        .pause(&me, "self", ApplyMode::Now, None)
        .expect("pause now");
    assert_eq!(out["state"]["state"], "paused");
    assert!(!root.node("self").unwrap().state.accepts_input());
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    let me = principal("self");
    let out = root
        .pause(&me, "self", ApplyMode::AfterOutput, None)
        .expect("pause after output");
    assert_eq!(out["state"]["state"], "pause_pending");
    assert!(root.node("self").unwrap().state.accepts_input());
    let settled = root.settle_after_output("self").expect("settle");
    assert_eq!(settled, ControlState::Paused {
        since: match settled {
            ControlState::Paused { since, .. } => since,
            _ => unreachable!(),
        },
        wake_reason: Some("after_output".into()),
    });
}

#[test]
fn paused_route_wakes_on_an_admitted_source_and_queues_the_rest() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    spawn(&mut root, "peer");
    let me = principal("self");
    let other = principal("peer");
    source(&mut root, "peer", "reply-1", SourceKind::ChildReply);
    source(&mut root, "peer", "msg-1", SourceKind::AgentMessage);
    let mut kinds = BTreeSet::new();
    kinds.insert(SourceKind::ChildReply);
    root.set_source_policy(&me, "self", kinds, BTreeSet::new())
        .expect("policy");
    root.pause(&me, "self", ApplyMode::Now, None).expect("pause");
    root.grant(&me, "self", "peer", {
        let mut rights = BTreeSet::new();
        rights.insert(ControlRight::Wake);
        rights
    })
    .expect("grant wake");
    let blocked = root
        .arrive("msg-1", "self", json!({}))
        .expect("arrive blocked");
    assert_eq!(blocked["admitted"], false);
    assert_eq!(blocked["resumed"], false);
    assert_eq!(blocked["state"]["state"], "paused");
    let woken = root
        .arrive("reply-1", "self", json!({}))
        .expect("arrive wake");
    assert_eq!(woken["admitted"], true);
    assert_eq!(woken["resumed"], true);
    assert_eq!(woken["state"]["state"], "running");
    let drained = root.drain_admitted("self", 8).expect("drain");
    let items = drained["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["source_id"], "reply-1");
    assert_eq!(root.node("self").unwrap().pending(), 1);
}

#[test]
fn stop_with_wake_sources_keeps_queue_and_terminal_stop_does_not() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    spawn(&mut root, "peer");
    let me = principal("self");
    let other = principal("peer");
    source(&mut root, "peer", "reply-1", SourceKind::ChildReply);
    source(&mut root, "peer", "reply-2", SourceKind::ChildReply);
    root.grant(&me, "self", "peer", {
        let mut rights = BTreeSet::new();
        rights.insert(ControlRight::Stop);
        rights
    })
    .expect("grant stop");
    let mut wake = BTreeSet::new();
    wake.insert("reply-1".to_owned());
    let out = root
        .stop(&other, "self", ApplyMode::Now, wake)
        .expect("stop with wake");
    assert_eq!(out["terminal"], false);
    let blocked = root.arrive("reply-2", "self", json!({})).expect("arrive");
    assert_eq!(blocked["admitted"], false);
    let woken = root.arrive("reply-1", "self", json!({})).expect("arrive");
    assert_eq!(woken["resumed"], true);
    assert_eq!(root.node("self").unwrap().pending(), 1);
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    let me = principal("self");
    let stopped = root
        .stop(&me, "self", ApplyMode::Now, BTreeSet::new())
        .expect("terminal stop");
    assert_eq!(stopped["terminal"], true);
    assert!(root.node("self").unwrap().state.is_terminal());
}

#[test]
fn sleep_is_a_pause_never_a_self_stop_and_keeps_arrivals() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    let me = principal("self");
    let out = root
        .sleep(&me, Some(30), Some("waiting".into()))
        .expect("sleep");
    assert_eq!(out["state"]["state"], "paused");
    let node = root.node("self").unwrap();
    assert!(!node.state.is_terminal());
    assert!(node.policy.is_open());
    let clock = out["clock_source"].as_str().unwrap().to_owned();
    let woke = root.arrive(&clock, "self", json!({})).expect("clock arrive");
    assert_eq!(woke["resumed"], true);
}

#[test]
fn unregistered_sources_cannot_arrive_or_wake() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    let me = principal("self");
    root.pause(&me, "self", ApplyMode::Now, None).expect("pause");
    let error = root
        .arrive("ghost", "self", json!({}))
        .expect_err("unregistered");
    assert_eq!(error.code, "unregistered_source");
    let mut ids = BTreeSet::new();
    ids.insert("ghost".to_owned());
    let error = root
        .set_source_policy(&me, "self", BTreeSet::new(), ids)
        .expect_err("unregistered policy");
    assert_eq!(error.status, 422);
}

#[test]
fn admin_grant_never_widens_and_revoke_removes() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    spawn(&mut root, "child");
    spawn(&mut root, "peer");
    let me = principal("self");
    let other = principal("peer");
    let mut pause = BTreeSet::new();
    pause.insert(ControlRight::Pause);
    root.grant(&principal("child"), "child", "peer", pause.clone())
        .expect("grant pause");
    assert_eq!(root.held(&other, "child"), pause);
    let mut stop = BTreeSet::new();
    stop.insert(ControlRight::Stop);
    let error = root
        .grant(&other, "child", "child", stop)
        .expect_err("peer has no grant right");
    assert_eq!(error.status, 403);
    root.revoke(&principal("child"), "child", "peer").expect("revoke");
    assert!(root.held(&other, "child").is_empty());
}

#[test]
fn a_route_without_rights_cannot_signal_another() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    spawn(&mut root, "child");
    let other = principal("self");
    let error = root
        .pause(&other, "child", ApplyMode::Now, None)
        .expect_err("no pause right");
    assert_eq!(error.status, 403);
}

#[test]
fn i_broadcast_reports_child_states_and_the_management_log() {
    let mut root = SignalState::new();
    spawn(&mut root, "root");
    spawn(&mut root, "boss");
    fork(&mut root, "root", "root", "kid", vec!["cb_1"]);
    let rootp = principal("root");
    root.pause(&rootp, "kid", ApplyMode::Now, None).expect("pause kid");
    let broadcast = root.i_broadcast("root", 8);

    let children = broadcast["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["route_id"], "kid");
    assert_eq!(children[0]["state"]["state"], "paused");
    let log = broadcast["management_log"].as_array().unwrap();
    assert!(log.len() >= 2);
    assert!(log.iter().all(|entry| entry["at"].as_u64().is_some()));
    let last = log.last().unwrap();
    assert_eq!(last["action"], "signal.pause");
}

#[test]
fn status_reports_limits_origin_and_current_state() {
    let mut root = SignalState::new();
    spawn(&mut root, "self");
    fork(&mut root, "self", "self", "kid", vec!["cb_1"]);
    let status = root.status("kid").expect("status");
    assert_eq!(status["depth"], 1);
    assert_eq!(status["parents"], json!(["self"]));
    assert_eq!(status["state"]["state"], "running");
    assert_eq!(status["limits"]["max_child_depth"], 8);
    assert_eq!(status["held_rights"].as_array().unwrap().len(), 6);
}
