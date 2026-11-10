#[path = "../src/ipc.rs"]
mod ipc;

use ipc::{IpcState, Lifecycle, Principal, RouteStatus, SignalAction, SocketAcl};
use serde_json::{Value, json};

fn principal(actor: &str, route: &str) -> Principal {
    Principal::new(actor, route).unwrap()
}

fn setup() -> (IpcState, Principal, Principal, String) {
    let mut state = IpcState::new();
    let owner = principal("alice", "agent-a");
    state
        .create_route("agent-a", "alice", vec!["alice".into()], &owner)
        .unwrap();
    state
        .record_checkpoint(
            &owner,
            "cp-1",
            "cut-1",
            "cb-1",
            1,
            json!({"history":"frozen-one","pvi":{"P":["old"]}}),
        )
        .unwrap();
    let acl = SocketAcl {
        message: vec!["agent-b".into()],
        signal: vec!["host-b".into()],
        spawn: vec!["agent-b".into()],
        read: vec!["reader".into()],
    };
    state
        .create_socket(&owner, "socket-1", "cp-1", acl)
        .unwrap();
    let sender = principal("bob", "agent-b");
    state
        .create_route("agent-b", "bob", vec!["bob".into()], &sender)
        .unwrap();
    let reader = principal("reader", "reader-route");
    state
        .create_route("reader-route", "reader", vec!["reader".into()], &reader)
        .unwrap();
    let controller = principal("host-b", "host-b-route");
    state
        .create_route("host-b-route", "host-b", vec!["host-b".into()], &controller)
        .unwrap();
    (state, owner, sender, "socket-1".into())
}

#[test]
fn route_identity_and_checkpoint_history_are_independent_and_frozen() {
    let (mut state, owner, sender, socket) = setup();
    let reader = principal("reader", "reader-route");
    assert_eq!(
        state.read_checkpoint(&reader, &socket).unwrap(),
        json!({"history":"frozen-one","pvi":{"P":["old"]}})
    );
    let denied_reader = principal("bob", "agent-b");
    assert!(state.read_checkpoint(&denied_reader, &socket).is_err());

    let peer = state
        .spawn_peer(
            &sender,
            &socket,
            "agent-peer",
            "req-peer",
            "msg-task",
            "solve it",
            Lifecycle::Volatile,
        )
        .unwrap();
    assert_ne!(peer, "agent-a");
    let fork = state.route(&peer).unwrap();
    assert_eq!(fork.origin_checkpoint(), Some("cp-1"));
    assert_eq!(
        fork.initial_snapshot(),
        &json!({"history":"frozen-one","pvi":{"P":["old"]}})
    );
    assert_eq!(fork.owner_actor_id(), "bob");
    assert!(
        fork.creators().is_empty(),
        "historical fork is not a blank-route parent-child creation"
    );
    assert_eq!(fork.inbox()[0].message.text, "solve it");
    assert_eq!(state.route("agent-a").unwrap().id(), owner.route_id);
}

#[test]
fn historical_checkpoint_can_be_registered_after_newer_cut_and_forked() {
    let mut state = IpcState::new();
    let owner = principal("alice", "agent-a");
    state
        .create_route("agent-a", "alice", vec!["alice".into()], &owner)
        .unwrap();
    state
        .record_checkpoint(
            &owner,
            "cp-new",
            "cut-new",
            "cb-new",
            9,
            json!({"history":"newer"}),
        )
        .unwrap();
    state
        .record_checkpoint(
            &owner,
            "cp-old",
            "cut-old",
            "cb-old",
            3,
            json!({"history":"older"}),
        )
        .unwrap();
    state
        .record_checkpoint(
            &owner,
            "cp-old-alias",
            "cut-old",
            "cb-old",
            3,
            json!({"history":"older"}),
        )
        .unwrap();
    assert!(
        state
            .record_checkpoint(
                &owner,
                "cp-old",
                "cut-old",
                "cb-old",
                3,
                json!({"history":"older"}),
            )
            .is_err()
    );

    state
        .create_socket(
            &owner,
            "old-socket",
            "cp-old",
            SocketAcl {
                message: vec![],
                signal: vec![],
                spawn: vec!["bob".into()],
                read: vec![],
            },
        )
        .unwrap();
    let sender = principal("bob", "agent-b");
    state
        .create_route("agent-b", "bob", vec![], &sender)
        .unwrap();
    let peer = state
        .spawn_peer(
            &sender,
            "old-socket",
            "fork-from-old",
            "req-old",
            "msg-old",
            "continue from old history",
            Lifecycle::Volatile,
        )
        .unwrap();
    assert_eq!(
        state.route(&peer).unwrap().origin_checkpoint(),
        Some("cp-old")
    );
    assert_eq!(
        state.route(&peer).unwrap().initial_snapshot(),
        &json!({"history":"older"})
    );
}

#[test]
fn message_is_ordered_idempotent_and_key_reuse_with_new_content_is_rejected() {
    let (mut state, _, sender, socket) = setup();
    let first = state
        .deliver_message(
            &sender,
            &socket,
            "req-1",
            "msg-1",
            "first clue",
            Lifecycle::Persistent,
            Some("task-a".into()),
            Some("phase-1".into()),
        )
        .unwrap();
    let again = state
        .deliver_message(
            &sender,
            &socket,
            "req-1",
            "msg-1",
            "first clue",
            Lifecycle::Persistent,
            Some("task-a".into()),
            Some("phase-1".into()),
        )
        .unwrap();
    assert!(!first.replayed);
    let stored_first = &state.route("agent-a").unwrap().inbox()[0].message;
    assert_eq!(stored_first.task_id.as_deref(), Some("task-a"));
    assert_eq!(stored_first.phase_id.as_deref(), Some("phase-1"));
    assert!(again.replayed);
    assert_eq!(first.receipt, again.receipt);
    let second = state
        .deliver_message(
            &sender,
            &socket,
            "req-1",
            "msg-2",
            "later clue",
            Lifecycle::Volatile,
            None,
            None,
        )
        .unwrap();
    assert!(second.receipt.sequence > first.receipt.sequence);
    assert!(
        state
            .deliver_message(
                &sender,
                &socket,
                "req-1",
                "msg-1",
                "changed clue",
                Lifecycle::Persistent,
                None,
                None
            )
            .is_err()
    );
    let inbox = state.route("agent-a").unwrap().inbox();
    assert_eq!(inbox.len(), 2);
    assert_eq!(inbox[0].message.message_id, "msg-1");
    assert_eq!(inbox[1].message.message_id, "msg-2");
    assert!(inbox.iter().all(|row| row.consumed_by_cut.is_none()));
}

#[test]
fn publication_is_previewed_without_consumption_then_pv_are_confirmed_once() {
    let (mut state, _, sender, socket) = setup();
    state
        .deliver_message(
            &sender,
            &socket,
            "req",
            "p-msg",
            "keep this fact",
            Lifecycle::Persistent,
            None,
            None,
        )
        .unwrap();
    state
        .deliver_message(
            &sender,
            &socket,
            "req",
            "v-msg",
            "next turn hint",
            Lifecycle::Volatile,
            None,
            None,
        )
        .unwrap();

    let preview = state.preview_publication("agent-a", "cut-2", true).unwrap();
    assert_eq!(preview.persistent_in_cb().len(), 1);
    assert_eq!(preview.volatile_next_wb().len(), 1);
    assert!(
        state
            .route("agent-a")
            .unwrap()
            .inbox()
            .iter()
            .all(|row| row.consumed_by_cut.is_none()),
        "preview is read-only"
    );

    let confirmed = state.confirm_publication(&preview, "cb-2").unwrap();
    assert!(!confirmed.replayed);
    assert_eq!(
        confirmed.receipt.persistent_in_cb[0].message.text,
        "keep this fact"
    );
    assert_eq!(
        confirmed.receipt.volatile_next_wb[0].message.text,
        "next turn hint"
    );
    assert_eq!(
        state
            .route("agent-a")
            .unwrap()
            .inbox()
            .iter()
            .map(|row| row.consumed_by_cut.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("cut-2"), Some("cut-2")]
    );

    let retried = state.confirm_publication(&preview, "cb-2").unwrap();
    assert!(retried.replayed);
    assert_eq!(confirmed.receipt, retried.receipt);
    assert!(state.confirm_publication(&preview, "different-cb").is_err());
    assert!(state.preview_publication("agent-a", "cut-2", true).is_err());
}

#[test]
fn non_visible_preview_does_not_consume_and_modified_plan_is_rejected() {
    let (mut state, _, sender, socket) = setup();
    state
        .deliver_message(
            &sender,
            &socket,
            "req",
            "msg",
            "message",
            Lifecycle::Persistent,
            None,
            None,
        )
        .unwrap();
    let hidden = state
        .preview_publication("agent-a", "cut-hidden", false)
        .unwrap();
    assert!(!hidden.eligible());
    assert!(hidden.messages().is_empty());
    assert!(state.confirm_publication(&hidden, "cb-hidden").is_err());

    let mut changed: Value = serde_json::to_value(
        state
            .preview_publication("agent-a", "cut-visible", true)
            .unwrap(),
    )
    .unwrap();
    changed["messages"][0]["message"]["text"] = json!("tampered");
    let changed: ipc::PublicationPreview = serde_json::from_value(changed).unwrap();
    assert!(state.confirm_publication(&changed, "cb-visible").is_err());
    assert!(
        state.route("agent-a").unwrap().inbox()[0]
            .consumed_by_cut
            .is_none()
    );
}

#[test]
fn message_signal_spawn_and_read_permissions_are_separate() {
    let (mut state, _, sender, socket) = setup();
    assert!(
        state
            .deliver_message(
                &principal("alice", "agent-a"),
                &socket,
                "r",
                "m",
                "no",
                Lifecycle::Persistent,
                None,
                None
            )
            .is_err()
    );
    assert!(
        state
            .signal(&sender, &socket, "s1", SignalAction::Pause)
            .is_err()
    );
    assert!(
        state
            .signal(
                &principal("intruder", "host-b-route"),
                &socket,
                "forged-signal",
                SignalAction::Stop,
            )
            .is_err()
    );
    let peer = state
        .spawn_peer(
            &sender,
            &socket,
            "peer-no-read",
            "request",
            "task",
            "work",
            Lifecycle::Volatile,
        )
        .unwrap();
    assert_eq!(
        state.route(&peer).unwrap().origin_checkpoint(),
        Some("cp-1")
    );
    assert!(
        state.read_checkpoint(&sender, &socket).is_err(),
        "spawn ACL does not grant direct checkpoint read"
    );
}

#[test]
fn signals_transition_only_with_signal_acl_and_stop_is_terminal() {
    let (mut state, _, _, socket) = setup();
    let controller = principal("host-b", "host-b-route");
    let paused = state
        .signal(&controller, &socket, "sig-1", SignalAction::Pause)
        .unwrap();
    assert!(paused.receipt.transitioned);
    assert_eq!(paused.receipt.sequence, 1);
    assert_eq!(
        state.route("agent-a").unwrap().status(),
        &RouteStatus::Waiting
    );
    let repeated = state
        .signal(&controller, &socket, "sig-1", SignalAction::Pause)
        .unwrap();
    assert!(repeated.replayed);
    assert_eq!(paused.receipt, repeated.receipt);

    let woken = state
        .signal(&controller, &socket, "sig-wake", SignalAction::Wake)
        .unwrap();
    assert!(woken.receipt.transitioned);
    assert_eq!(woken.receipt.resulting_status, RouteStatus::Running);
    let paused_again = state
        .signal(&controller, &socket, "sig-pause-again", SignalAction::Pause)
        .unwrap();
    assert!(paused_again.receipt.transitioned);
    let resumed = state
        .signal(&controller, &socket, "sig-2", SignalAction::Resume)
        .unwrap();
    assert!(resumed.receipt.transitioned);
    assert_eq!(resumed.receipt.resulting_status, RouteStatus::Running);
    let stopped = state
        .signal(&controller, &socket, "sig-3", SignalAction::Stop)
        .unwrap();
    assert!(stopped.receipt.transitioned);
    let late_wake = state
        .signal(&controller, &socket, "sig-4", SignalAction::Wake)
        .unwrap();
    assert!(!late_wake.receipt.transitioned);
    assert_eq!(late_wake.receipt.resulting_status, RouteStatus::Stopped);
    assert!(
        state
            .deliver_message(
                &principal("bob", "agent-b"),
                &socket,
                "late-req",
                "late-msg",
                "late result",
                Lifecycle::Volatile,
                None,
                None
            )
            .unwrap()
            .receipt
            .sequence
            > stopped.receipt.sequence
    );
    assert_eq!(
        state.route("agent-a").unwrap().status(),
        &RouteStatus::Stopped,
        "late message does not revive stopped Agent"
    );
    assert!(
        state
            .preview_publication("agent-a", "cut-after-stop", true)
            .is_err()
    );
}

#[test]
fn checkpoint_socket_cannot_bind_another_routes_history_and_serializes_cleanly() {
    let (mut state, owner, _, _) = setup();
    let other = principal("carol", "agent-c");
    state
        .create_route("agent-c", "carol", vec!["carol".into()], &other)
        .unwrap();
    state
        .record_checkpoint(
            &other,
            "cp-c",
            "cut-c",
            "cb-c",
            1,
            json!({"history":"other"}),
        )
        .unwrap();
    let bad_acl = SocketAcl::empty();
    assert!(
        state
            .create_socket(&owner, "bad-socket", "cp-c", bad_acl)
            .is_err()
    );

    let encoded = serde_json::to_vec(&state).unwrap();
    let decoded: IpcState = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.route("agent-a").unwrap().id(), "agent-a");
    assert_eq!(
        decoded.checkpoint("cp-1").unwrap().snapshot(),
        &json!({"history":"frozen-one","pvi":{"P":["old"]}})
    );
}
