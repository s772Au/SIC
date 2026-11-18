pub mod error {
    pub use sic_core_rust::error::*;
}
pub mod storage {
    pub use sic_core_rust::storage::*;
}
pub mod toolset {
    pub use sic_core_rust::toolset::*;
}
pub mod prefix {
    pub use sic_core_rust::prefix::*;
}
pub mod core {
    pub use sic_core_rust::core::*;
}
pub mod transparent {
    pub use sic_core_rust::transparent::*;
}
#[path = "../src/tool_dispatch.rs"]
mod tool_dispatch;
#[path = "../src/tool_store.rs"]
pub mod tool_store;

use serde_json::{Value, json};
use sic_core_rust::{
    storage::Store,
    toolset::{AclGrant, ToolAcl, ToolDescriptor},
};
use tool_dispatch::{
    AuthenticatedWorkerScope, ToolDispatch, complete_worker_response_in_transaction,
    dispatch_model_tool_in_transaction, load_tool_result_in_transaction,
    persist_tool_result_in_transaction,
};
use tool_store::ToolStore;

const ACTOR: &str = "host-worker";
const CHAIN: &str = "chain-tool-dispatch";

fn scope(allow: bool) -> AuthenticatedWorkerScope {
    AuthenticatedWorkerScope::from_authenticated_host(ACTOR, CHAIN, allow).unwrap()
}

fn descriptor(name: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_owned(),
        description: format!("{name} integration fixture"),
        schema: json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}),
        program: None,
    }
}

fn acl() -> ToolAcl {
    let grants = || vec![AclGrant::new(ACTOR, "fixture")];
    ToolAcl::new(grants(), grants(), grants())
}

async fn configured() -> Store {
    let db = Store::memory().await.unwrap();
    ToolStore::new(db.clone())
        .configure_from_host(CHAIN, ACTOR, acl(), vec![(descriptor("fixture"), false)])
        .await
        .unwrap();
    db
}

async fn dispatch(
    db: &Store,
    scope: &AuthenticatedWorkerScope,
    name: &str,
    args: Value,
) -> sic_core_rust::error::Result<ToolDispatch> {
    let mut tx = db.begin().await?;
    let result = dispatch_model_tool_in_transaction(&mut tx, scope, name, &args).await?;
    tx.commit().await?;
    Ok(result)
}

#[tokio::test]
async fn parses_fixed_operations_and_rejects_model_supplied_identity_or_unknown_fields() {
    let db = configured().await;
    let denied = dispatch(
        &db,
        &scope(false),
        "sic_tool_update",
        json!({"select":["fixture"]}),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.status, 403);

    let bad = dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"select":["fixture"],"actor":"attacker"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad.status, 422);
    let unknown = dispatch(&db, &scope(true), "sic_tool_export", json!({}))
        .await
        .unwrap_err();
    // Original host names now enter the native dispatch path; unknown names
    // still cannot acquire a host execution lease without an execute grant.
    assert_eq!(unknown.status, 403);

    // Selecting nothing and folding nothing is a shape error, not a no-op.
    let empty = dispatch(&db, &scope(true), "sic_tool_update", json!({}))
        .await
        .unwrap_err();
    assert_eq!(empty.status, 422);

    let selected = dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"select":["fixture"]}),
    )
    .await
    .unwrap();
    let ToolDispatch::Updated { update } = selected else {
        panic!("expected an update result")
    };
    assert_eq!(update.added, vec!["fixture".to_owned()]);
    assert!(update.message.contains("Added fixture."));

    let folded = dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"fold":["fixture"]}),
    )
    .await
    .unwrap();
    let ToolDispatch::Updated { update } = folded else {
        panic!("expected an update result")
    };
    assert_eq!(update.folded, vec!["fixture".to_owned()]);
    assert!(update.message.contains("Folded fixture as you requested."));
}

#[tokio::test]
async fn update_handles_a_large_catalog_and_enforces_capacity() {
    let db = Store::memory().await.unwrap();
    let names = (0..130)
        .map(|index| format!("catalog-{index:03}"))
        .collect::<Vec<_>>();
    let grants = names
        .iter()
        .map(|name| AclGrant::new(ACTOR, name.clone()))
        .collect::<Vec<_>>();
    ToolStore::new(db.clone())
        .configure_from_host(
            CHAIN,
            ACTOR,
            ToolAcl::new(grants.clone(), grants.clone(), grants.clone()),
            names
                .iter()
                .cloned()
                .chain(std::iter::once("catalog-hidden".to_owned()))
                .map(|name| (descriptor(&name), false))
                .collect(),
        )
        .await
        .unwrap();
    let first_twelve = names.iter().take(12).cloned().collect::<Vec<_>>();
    let selected = dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"select":first_twelve}),
    )
    .await
    .unwrap();
    let ToolDispatch::Updated { update } = selected else {
        panic!("expected an update result")
    };
    assert_eq!(update.added, first_twelve);
    assert!(update.evicted.is_empty());

    // The request accepts at most 64 names per field, so the ceiling is
    // reached in batches; the host folds least-recently-used tools and names them.
    let mut bulk_update = None;
    for batch in names.chunks(64) {
        let bulk = dispatch(
            &db,
            &scope(true),
            "sic_tool_update",
            json!({"select":batch}),
        )
        .await
        .unwrap();
        let ToolDispatch::Updated { update } = bulk else {
            panic!("expected an update result")
        };
        bulk_update = Some(update);
    }
    let update = bulk_update.expect("at least one batch ran");
    assert!(!update.evicted.is_empty());
    assert!(update.message.contains("Also folded"));

    // A name outside the catalog is reported, never silently dropped.
    let ghost = dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"select":["catalog-hidden"]}),
    )
    .await
    .unwrap();
    let ToolDispatch::Updated { update } = ghost else {
        panic!("expected an update result")
    };
    assert_eq!(update.rejected.len(), 1);
    assert_eq!(update.rejected[0].reason, "expansion_denied");

    for invalid in [
        json!({"select":"catalog-000"}),
        json!({"select":["catalog-000","catalog-000"]}),
        json!({"select":[""]}),
        json!({"select":["catalog-000"],"extend":[]}),
    ] {
        assert_eq!(
            dispatch(&db, &scope(true), "sic_tool_update", invalid)
                .await
                .unwrap_err()
                .status,
            422
        );
    }
    db.close().await;
}

#[tokio::test]
async fn call_auto_expands_yields_host_lease_and_result_persists_before_release() {
    let db = configured().await;
    let host_scope = scope(true);
    assert_eq!(host_scope.actor(), ACTOR);
    assert_eq!(host_scope.chain_id(), CHAIN);
    let pending = dispatch(
        &db,
        &host_scope,
        "sic_tool_call",
        json!({
            "name":"fixture", "arguments":{"value":"hello"}
        }),
    )
    .await
    .unwrap();
    let ToolDispatch::HostExecutionRequired {
        lease,
        descriptor,
        arguments,
        ..
    } = pending
    else {
        panic!("expected host execution handoff")
    };
    assert_eq!(descriptor.name, "fixture");
    assert_eq!(arguments, json!({"value":"hello"}));

    // The model request only admitted the call. A trusted external host runner
    // now supplies the observed result in the same transaction that releases
    // the lease.
    let result = json!({"value":"world"});
    let mut rollback_tx = db.begin().await.unwrap();
    persist_tool_result_in_transaction(&mut rollback_tx, &host_scope, &lease, true, &result)
        .await
        .unwrap();
    rollback_tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let active = ToolStore::active_leases_in_transaction(&mut tx, CHAIN, ACTOR)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert!(
        load_tool_result_in_transaction(&mut tx, &host_scope, lease.id())
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let persisted = persist_tool_result_in_transaction(&mut tx, &host_scope, &lease, true, &result)
        .await
        .unwrap();
    assert_eq!(persisted.result, result);
    let loaded = load_tool_result_in_transaction(&mut tx, &host_scope, lease.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, persisted);
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let active = ToolStore::active_leases_in_transaction(&mut tx, CHAIN, ACTOR)
        .await
        .unwrap();
    assert!(active.is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn worker_response_completion_advances_once_and_rolls_back_atomically() {
    let db = configured().await;
    dispatch(
        &db,
        &scope(true),
        "sic_tool_update",
        json!({"select":["fixture"]}),
    )
    .await
    .unwrap();
    let host_scope = scope(true);
    let mut tx = db.begin().await.unwrap();
    let first = complete_worker_response_in_transaction(&mut tx, &host_scope, "response-1")
        .await
        .unwrap();
    assert!(first.advanced);
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let first = complete_worker_response_in_transaction(&mut tx, &host_scope, "response-1")
        .await
        .unwrap();
    assert!(first.advanced);
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let repeated = complete_worker_response_in_transaction(&mut tx, &host_scope, "response-1")
        .await
        .unwrap();
    assert!(!repeated.advanced);
    tx.commit().await.unwrap();
}
