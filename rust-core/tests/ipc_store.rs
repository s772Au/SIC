#[path = "../src/error.rs"]
mod error;
#[path = "../src/ipc.rs"]
mod ipc;
#[path = "../src/ipc_store.rs"]
mod ipc_store;
#[path = "../src/storage.rs"]
mod storage;

use error::CoreError;
use ipc::{IpcState, Lifecycle, Principal, SocketAcl};
use ipc_store::IpcStore;
use serde_json::json;
use std::path::Path;
use storage::Store;

fn principal(actor: &str, route: &str) -> Principal {
    Principal::new(actor, route).unwrap()
}

async fn new_file_store(path: &Path) -> Store {
    Store::create(path).await.unwrap()
}

async fn seeded_ipc(store: Store, namespace: &str) -> IpcStore {
    let ipc = IpcStore::new(store, namespace).unwrap();
    ipc.transact(|state| {
        let alice = principal("alice", "route-a");
        state
            .create_route("route-a", "alice", vec![], &alice)
            .map_err(CoreError::conflict)?;
        state
            .record_checkpoint(
                &alice,
                "cp-a",
                "cut-a",
                "cb-a",
                1,
                json!({"frozen":"checkpoint-a"}),
            )
            .map_err(CoreError::conflict)?;
        state
            .create_socket(
                &alice,
                "socket-a",
                "cp-a",
                SocketAcl {
                    message: vec!["route-b".into()],
                    signal: vec!["route-b".into()],
                    spawn: vec!["route-b".into()],
                    read: vec!["route-b".into()],
                },
            )
            .map_err(CoreError::conflict)?;
        let bob = principal("bob", "route-b");
        state
            .create_route("route-b", "bob", vec![], &bob)
            .map_err(CoreError::conflict)?;
        let peer = state
            .spawn_peer(
                &bob,
                "socket-a",
                "peer-b",
                "req-peer",
                "task-peer",
                "explore frozen point",
                Lifecycle::Volatile,
            )
            .map_err(CoreError::conflict)?;
        assert_eq!(peer, "peer-b");
        Ok(())
    })
    .await
    .unwrap();
    ipc
}

#[tokio::test]
async fn ipc_aggregate_is_isolated_and_recovers_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ipc.sqlite");
    let store = new_file_store(&path).await;
    let ipc = seeded_ipc(store.clone(), "tenant-a").await;
    let recovered_before_close = ipc.load().await.unwrap();
    assert_eq!(
        recovered_before_close
            .route("peer-b")
            .unwrap()
            .initial_snapshot(),
        &json!({"frozen":"checkpoint-a"})
    );
    assert_eq!(
        recovered_before_close
            .route("peer-b")
            .unwrap()
            .inbox()
            .len(),
        1
    );

    let other_namespace = IpcStore::new(store.clone(), "tenant-b").unwrap();
    assert!(
        other_namespace
            .load()
            .await
            .unwrap()
            .route("route-a")
            .is_none()
    );
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    let recovered = IpcStore::new(reopened, "tenant-a")
        .unwrap()
        .load()
        .await
        .unwrap();
    assert!(recovered.route("route-a").is_some());
    assert!(recovered.route("route-b").is_some());
    assert_eq!(
        recovered.route("peer-b").unwrap().origin_checkpoint(),
        Some("cp-a")
    );
    assert_eq!(
        recovered.socket("socket-a").unwrap().checkpoint_id(),
        "cp-a"
    );
}

#[tokio::test]
async fn failed_operation_rolls_back_all_ipc_state_changes() {
    let store = Store::memory().await.unwrap();
    let ipc = IpcStore::new(store, "rollback").unwrap();
    let result: error::Result<()> = ipc
        .transact(|state| {
            let owner = principal("alice", "temporary");
            state
                .create_route("temporary", "alice", vec![], &owner)
                .map_err(CoreError::conflict)?;
            Err(CoreError::invalid(
                "force rollback after in-memory mutation",
            ))
        })
        .await;
    assert!(result.is_err());
    assert!(ipc.load().await.unwrap().route("temporary").is_none());
}

#[tokio::test]
async fn ipc_can_share_caller_transaction_and_roll_back_with_other_writes() {
    let store = Store::memory().await.unwrap();
    let ipc = IpcStore::new(store.clone(), "shared-tx").unwrap();

    let mut tx = store.begin().await.unwrap();
    sqlx::query("INSERT INTO boards(id,state) VALUES('board-committed','{}')")
        .execute(&mut *tx)
        .await
        .unwrap();
    ipc.transact_in_transaction(&mut tx, |state| {
        let owner = principal("alice", "committed-route");
        state
            .create_route("committed-route", "alice", vec![], &owner)
            .map_err(CoreError::conflict)?;
        Ok(())
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(ipc.load().await.unwrap().route("committed-route").is_some());

    let mut tx = store.begin().await.unwrap();
    sqlx::query("INSERT INTO boards(id,state) VALUES('board-rolled-back','{}')")
        .execute(&mut *tx)
        .await
        .unwrap();
    ipc.transact_in_transaction(&mut tx, |state| {
        let owner = principal("alice", "rolled-back-route");
        state
            .create_route("rolled-back-route", "alice", vec![], &owner)
            .map_err(CoreError::conflict)?;
        Ok(())
    })
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        ipc.load()
            .await
            .unwrap()
            .route("rolled-back-route")
            .is_none()
    );
    let mut check = store.begin().await.unwrap();
    let rolled_back: Option<String> =
        sqlx::query_scalar("SELECT id FROM boards WHERE id='board-rolled-back'")
            .fetch_optional(&mut *check)
            .await
            .unwrap();
    check.commit().await.unwrap();
    assert!(rolled_back.is_none());
}

#[tokio::test]
async fn parallel_duplicate_deliveries_commit_one_ordered_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ipc-concurrent.sqlite");
    let base = new_file_store(&path).await;
    let ipc = seeded_ipc(base.clone(), "tenant").await;
    let clients = (0..6)
        .map(|_| async {
            let independent_pool = Store::open(&path).await.unwrap();
            IpcStore::new(independent_pool, "tenant").unwrap()
        })
        .collect::<Vec<_>>();
    let clients = futures_util::future::join_all(clients).await;
    let writes = clients.into_iter().map(|client| async move {
        client
            .transact(|state| {
                state
                    .deliver_message(
                        &principal("bob", "route-b"),
                        "socket-a",
                        "req-parallel",
                        "msg-parallel",
                        "same payload",
                        Lifecycle::Persistent,
                        Some("task-1".into()),
                        Some("phase-1".into()),
                    )
                    .map_err(CoreError::conflict)
            })
            .await
            .unwrap()
    });
    let outcomes = futures_util::future::join_all(writes).await;
    assert_eq!(outcomes.iter().filter(|result| !result.replayed).count(), 1);
    assert_eq!(outcomes.iter().filter(|result| result.replayed).count(), 5);
    assert!(
        outcomes
            .iter()
            .all(|result| result.receipt == outcomes[0].receipt)
    );
    let final_state = ipc.load().await.unwrap();
    let inbox = final_state.route("route-a").unwrap().inbox();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].message.task_id.as_deref(), Some("task-1"));
    assert_eq!(inbox[0].message.phase_id.as_deref(), Some("phase-1"));
}

#[tokio::test]
async fn concurrent_identity_collision_with_different_payload_is_rejected_without_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ipc-conflict.sqlite");
    let base = new_file_store(&path).await;
    let ipc = seeded_ipc(base.clone(), "tenant").await;
    let client_a = IpcStore::new(Store::open(&path).await.unwrap(), "tenant").unwrap();
    let client_b = IpcStore::new(Store::open(&path).await.unwrap(), "tenant").unwrap();
    let write = |client: IpcStore, text: &'static str| async move {
        client
            .transact(|state| {
                state
                    .deliver_message(
                        &principal("bob", "route-b"),
                        "socket-a",
                        "req-collision",
                        "msg-collision",
                        text,
                        Lifecycle::Persistent,
                        None,
                        None,
                    )
                    .map_err(CoreError::conflict)
            })
            .await
    };
    let (a, b) = tokio::join!(write(client_a, "payload A"), write(client_b, "payload B"));
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "exactly one distinct payload wins the key"
    );
    let state: IpcState = ipc.load().await.unwrap();
    let inbox = state.route("route-a").unwrap().inbox();
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0].message.text == "payload A" || inbox[0].message.text == "payload B");
}
