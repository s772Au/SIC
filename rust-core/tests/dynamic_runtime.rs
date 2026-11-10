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
        "sic_tool_search",
        json!({"query":"fixture"}),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.status, 403);

    let bad = dispatch(
        &db,
        &scope(true),
        "sic_tool_search",
        json!({"query":"fixture","actor":"attacker"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad.status, 422);
    let unknown = dispatch(&db, &scope(true), "sic_tool_export", json!({}))
        .await
        .unwrap_err();
    assert_eq!(unknown.status, 501);

    let found = dispatch(
        &db,
        &scope(true),
        "sic_tool_search",
        json!({"query":"fixture"}),
    )
    .await
    .unwrap();
    assert!(
        matches!(found, ToolDispatch::Search { tools, offset, total, next_offset }
        if tools.len() == 1 && tools[0].name == "fixture" && offset == 0 && total == 1 && next_offset.is_none())
    );
    let expanded = dispatch(
        &db,
        &scope(true),
        "sic_tool_expand",
        json!({"name":"fixture"}),
    )
    .await
    .unwrap();
    assert!(matches!(expanded, ToolDispatch::Expanded { .. }));
    let reclaimed = dispatch(
        &db,
        &scope(true),
        "sic_tool_reclaim",
        json!({"name":"fixture"}),
    )
    .await
    .unwrap();
    assert_eq!(reclaimed, ToolDispatch::Reclaimed { reclaimed: true });
}

#[tokio::test]
async fn search_paginates_large_discoverable_catalog_and_honors_acl() {
    let db = Store::memory().await.unwrap();
    let names = (0..130)
        .map(|index| format!("catalog-{index:03}"))
        .collect::<Vec<_>>();
    let grants = names
        .iter()
        .map(|name| AclGrant::new(ACTOR, name.clone()))
        .collect::<Vec<_>>();
    let expanded = ToolStore::new(db.clone())
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
        .await;
    expanded.unwrap();
    let first = dispatch(
        &db,
        &scope(true),
        "sic_tool_search",
        json!({"query":"catalog"}),
    )
    .await
    .unwrap();
    let ToolDispatch::Search {
        tools,
        offset,
        total,
        next_offset,
    } = first
    else {
        panic!("expected paginated search results")
    };
    assert_eq!(offset, 0);
    assert_eq!(tools.len(), 20);
    assert_eq!(total, 130);
    assert_eq!(next_offset, Some(20));
    assert!(tools.iter().all(|tool| tool.name != "catalog-hidden"));

    let mut pages = tools;
    let mut next = next_offset;
    while let Some(offset) = next {
        let page = dispatch(
            &db,
            &scope(true),
            "sic_tool_search",
            json!({"query":"catalog","offset":offset,"limit":100}),
        )
        .await
        .unwrap();
        let ToolDispatch::Search {
            tools,
            offset: actual_offset,
            total: actual_total,
            next_offset,
        } = page
        else {
            panic!("expected paginated search results")
        };
        assert_eq!(actual_offset, offset);
        assert_eq!(actual_total, 130);
        pages.extend(tools);
        next = next_offset;
    }
    assert_eq!(pages.len(), 130);
    assert_eq!(
        pages
            .iter()
            .map(|tool| &tool.name)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        130
    );
    assert!(!pages.iter().any(|tool| tool.name == "catalog-hidden"));

    for invalid in [
        json!({"query":"catalog","offset":-1}),
        json!({"query":"catalog","offset":"1"}),
        json!({"query":"catalog","offset":1.5}),
        json!({"query":"catalog","limit":0}),
        json!({"query":"catalog","limit":101}),
        json!({"query":"catalog","limit":"20"}),
    ] {
        assert_eq!(
            dispatch(&db, &scope(true), "sic_tool_search", invalid)
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
        "sic_tool_expand",
        json!({"name":"fixture"}),
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
