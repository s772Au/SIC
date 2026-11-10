pub mod error {
    pub use sic_core_rust::error::*;
}
pub mod storage {
    pub use sic_core_rust::storage::*;
}
#[path = "../src/board.rs"]
pub mod board;

use board::{BoardAcl, BoardActor, BoardService, TaskStatus};
use serde_json::{Value, json};
use sic_core_rust::storage::Store;

const BOARD: &str = "team-board";
fn actor(name: &str) -> BoardActor {
    BoardActor::from_authenticated_host(name).unwrap()
}
fn acl(readers: &[&str], writers: &[&str], claimers: &[&str], managers: &[&str]) -> BoardAcl {
    BoardAcl::new(
        readers.iter().copied(),
        writers.iter().copied(),
        claimers.iter().copied(),
        managers.iter().copied(),
    )
    .unwrap()
}

async fn configured(store: Store, grants: BoardAcl) -> BoardService {
    let service = BoardService::new(store);
    service
        .configure_from_host(BOARD, &actor("host"), grants)
        .await
        .unwrap();
    service
}

async fn text(
    store: &Store,
    by: &BoardActor,
    revision: u64,
    key: &str,
    value: &str,
) -> sic_core_rust::error::Result<Value> {
    let mut tx = store.begin().await?;
    let result =
        BoardService::publish_text_in_transaction(&mut tx, by, BOARD, revision, key, value).await?;
    tx.commit().await?;
    Ok(result)
}

#[tokio::test]
async fn multi_agent_entries_are_revisioned_idempotent_and_audited() {
    let store = Store::memory().await.unwrap();
    let service = configured(
        store.clone(),
        acl(
            &["alice", "bob", "host"],
            &["alice", "bob"],
            &["alice", "bob"],
            &["host"],
        ),
    )
    .await;
    let alice = actor("alice");
    let bob = actor("bob");
    let first = text(
        &store,
        &alice,
        1,
        "notice-1",
        "Agent A: checkpoint completed",
    )
    .await
    .unwrap();
    assert_eq!(first["revision"], 2);
    let retry = text(
        &store,
        &alice,
        1,
        "notice-1",
        "Agent A: checkpoint completed",
    )
    .await
    .unwrap();
    assert_eq!(retry, first);
    let mut tx = store.begin().await.unwrap();
    let structured = BoardService::publish_structured_in_transaction(
        &mut tx,
        &bob,
        BOARD,
        2,
        "notice-2",
        &json!({"kind":"result","count":3}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(structured["revision"], 3);

    let mut tx = store.begin().await.unwrap();
    let snapshot = BoardService::read_latest_i_in_transaction(&mut tx, &alice, BOARD)
        .await
        .unwrap();
    let history = BoardService::read_history_in_transaction(&mut tx, &alice, BOARD, 0, 20)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(snapshot["revision"], 3);
    assert_eq!(snapshot["entries"].as_array().unwrap().len(), 2);
    assert_eq!(history.len(), 3); // initial ACL/configuration event plus two publishes
    assert_eq!(history[0].action, "configure");
    assert_eq!(history[1].action, "entry.publish_text");
    assert_eq!(history[2].actor, "bob");
    drop(service);
}

#[tokio::test]
async fn task_claim_is_competing_cas_and_task_actions_have_separate_grants() {
    let store = Store::memory().await.unwrap();
    let _service = configured(
        store.clone(),
        acl(
            &["alice", "bob", "host"],
            &["alice"],
            &["alice", "bob"],
            &["host"],
        ),
    )
    .await;
    let alice = actor("alice");
    let bob = actor("bob");
    let host = actor("host");
    let mut tx = store.begin().await.unwrap();
    let created = BoardService::create_task_in_transaction(
        &mut tx,
        &alice,
        BOARD,
        1,
        "task-create",
        "publish summary",
        &json!({"target":"main"}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();
    let (a, b) = tokio::join!(
        claim(&store, &alice, &id, 2, "claim-a"),
        claim(&store, &bob, &id, 2, "claim-b"),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let result = a.or(b).unwrap();
    assert_eq!(result["task"]["status"], "claimed");
    assert_eq!(result["revision"], 3);

    let stale = text(&store, &alice, 2, "stale-entry", "stale")
        .await
        .unwrap_err();
    assert_eq!(stale.code, "conflict");
    let unauthorized = claim(&store, &host, &id, 3, "claim-host")
        .await
        .unwrap_err();
    assert_eq!(unauthorized.code, "forbidden");

    let mut tx = store.begin().await.unwrap();
    let released =
        BoardService::release_task_in_transaction(&mut tx, &host, BOARD, &id, 3, "release")
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(released["revision"], 4);
    assert_eq!(released["task"]["status"], "open");

    let mut tx = store.begin().await.unwrap();
    let denied = BoardService::transition_task_in_transaction(
        &mut tx,
        &alice,
        BOARD,
        &id,
        4,
        "transition-denied",
        TaskStatus::Cancelled,
    )
    .await
    .unwrap_err();
    assert_eq!(denied.code, "forbidden");
    tx.rollback().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    let cancelled = BoardService::transition_task_in_transaction(
        &mut tx,
        &host,
        BOARD,
        &id,
        4,
        "transition",
        TaskStatus::Cancelled,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cancelled["task"]["status"], "cancelled");
}

async fn claim(
    store: &Store,
    by: &BoardActor,
    task: &str,
    revision: u64,
    key: &str,
) -> sic_core_rust::error::Result<Value> {
    let mut tx = store.begin().await?;
    match BoardService::claim_task_in_transaction(&mut tx, by, BOARD, task, revision, key).await {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

#[tokio::test]
async fn acl_revocation_changes_latest_i_and_stale_publication_snapshot_is_rejected() {
    let store = Store::memory().await.unwrap();
    let service = configured(
        store.clone(),
        acl(&["alice", "host"], &["alice"], &["alice"], &["host"]),
    )
    .await;
    let alice = actor("alice");
    let host = actor("host");
    text(
        &store,
        &alice,
        1,
        "alice-entry",
        "visible before revocation",
    )
    .await
    .unwrap();
    let mut tx = store.begin().await.unwrap();
    let old_i = BoardService::read_latest_i_in_transaction(&mut tx, &alice, BOARD)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let updated = service
        .update_acl_from_host(
            &host,
            BOARD,
            2,
            "revoke-alice",
            acl(&["host"], &[], &[], &["host"]),
        )
        .await
        .unwrap();
    assert_eq!(updated["revision"], 3);
    let mut tx = store.begin().await.unwrap();
    let denied = BoardService::read_latest_i_in_transaction(&mut tx, &alice, BOARD)
        .await
        .unwrap_err();
    assert_eq!(denied.code, "forbidden");
    tx.rollback().await.unwrap();

    let mut tx = store.begin().await.unwrap();
    // Test-only CB sink: stale I is rejected before insertion, while a fresh
    // I read and the fixture CB write share this caller-owned transaction.
    sqlx::query(
        "CREATE TEMP TABLE test_cb_i_snapshots (cb_id TEXT PRIMARY KEY, i_json TEXT NOT NULL)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    let stale = BoardService::read_latest_i_for_publish_in_transaction(
        &mut tx,
        &host,
        BOARD,
        Some(old_i["revision"].as_u64().unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "conflict");
    let stale_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM test_cb_i_snapshots")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(stale_rows, 0);
    let current =
        BoardService::read_latest_i_for_publish_in_transaction(&mut tx, &host, BOARD, Some(3))
            .await
            .unwrap();
    sqlx::query("INSERT INTO test_cb_i_snapshots(cb_id,i_json) VALUES('fixture-cb',?)")
        .bind(serde_json::to_string(&current).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT i_json FROM test_cb_i_snapshots WHERE cb_id='fixture-cb'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(current["revision"], 3);
    assert_ne!(current["digest"], old_i["digest"]);
    assert_eq!(stored["revision"], 3);
    assert_ne!(stored["digest"], old_i["digest"]);
    let attempted = text(&store, &alice, 3, "revoked-write", "should fail")
        .await
        .unwrap_err();
    assert_eq!(attempted.code, "forbidden");
}

#[tokio::test]
async fn board_state_and_audit_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("board-store.sqlite");
    let store = Store::create(&path).await.unwrap();
    let service = configured(
        store.clone(),
        acl(&["host"], &["host"], &["host"], &["host"]),
    )
    .await;
    let host = actor("host");
    text(&store, &host, 1, "persisted", "durable notice")
        .await
        .unwrap();
    drop(service);
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    let mut tx = reopened.begin().await.unwrap();
    let latest = BoardService::read_latest_i_in_transaction(&mut tx, &host, BOARD)
        .await
        .unwrap();
    let history = BoardService::read_history_in_transaction(&mut tx, &host, BOARD, 0, 10)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(latest["revision"], 2);
    assert_eq!(latest["entries"][0]["content"], "durable notice");
    assert_eq!(history.len(), 2);
    reopened.close().await;
}

#[tokio::test]
async fn transaction_rollback_does_not_publish_an_entry() {
    let store = Store::memory().await.unwrap();
    let _service = configured(
        store.clone(),
        acl(&["host"], &["host"], &["host"], &["host"]),
    )
    .await;
    let host = actor("host");
    let mut tx = store.begin().await.unwrap();
    BoardService::publish_text_in_transaction(&mut tx, &host, BOARD, 1, "rolled-back", "temporary")
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    let latest = BoardService::read_latest_i_in_transaction(&mut tx, &host, BOARD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(latest["revision"], 1);
    assert!(latest["entries"].as_array().unwrap().is_empty());
}
