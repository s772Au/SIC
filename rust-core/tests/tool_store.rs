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

#[path = "../src/tool_store.rs"]
mod tool_store;

use serde_json::{Value, json};
use sic_core_rust::{
    storage::Store,
    toolset::{AclGrant, ToolAcl, ToolDescriptor, ToolSet},
};
use tool_store::ToolStore;

const CHAIN: &str = "chain-tool-store";
const ACTOR: &str = "worker-host";

fn acl(names: &[&str]) -> ToolAcl {
    let grants = || {
        names
            .iter()
            .map(|name| AclGrant::new(ACTOR, *name))
            .collect()
    };
    ToolAcl::new(grants(), grants(), grants())
}

fn descriptor(name: impl Into<String>) -> ToolDescriptor {
    let name = name.into();
    ToolDescriptor {
        description: format!("Persistent test description for {name}"),
        schema: json!({"type":"object","properties":{"value":{"type":"string"}}}),
        name,
    }
}

fn tools(names: &[String]) -> Vec<(ToolDescriptor, bool)> {
    names
        .iter()
        .map(|name| (descriptor(name.clone()), false))
        .collect()
}

async fn configured(store: Store, names: &[&str]) -> ToolStore {
    let service = ToolStore::new(store);
    service
        .configure_from_host(
            CHAIN,
            ACTOR,
            acl(names),
            names
                .iter()
                .map(|name| (descriptor(*name), false))
                .collect(),
        )
        .await
        .unwrap();
    service
}

#[tokio::test]
async fn host_configured_acl_persists_and_model_actions_cannot_change_it() {
    let database = Store::memory().await.unwrap();
    let service = ToolStore::new(database);
    service
        .configure_from_host(
            CHAIN,
            ACTOR,
            ToolAcl::new(
                vec![AclGrant::new(ACTOR, "allowed")],
                vec![AclGrant::new(ACTOR, "allowed")],
                vec![AclGrant::new(ACTOR, "allowed")],
            ),
            vec![
                (descriptor("allowed"), false),
                (descriptor("hidden"), false),
            ],
        )
        .await
        .unwrap();
    let found = service
        .search_descriptions(CHAIN, ACTOR, "allowed")
        .await
        .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        vec!["allowed"]
    );
    assert!(
        service
            .search_descriptions(CHAIN, ACTOR, "hidden")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        service
            .request_expansion(CHAIN, ACTOR, "hidden")
            .await
            .unwrap_err()
            .status,
        403
    );
    service
        .request_expansion(CHAIN, ACTOR, "allowed")
        .await
        .unwrap();
    let lease = service
        .begin_execution(CHAIN, ACTOR, "allowed")
        .await
        .unwrap()
        .lease;
    service
        .finish_execution(CHAIN, ACTOR, &lease)
        .await
        .unwrap();
    assert!(
        service
            .request_expansion(CHAIN, "other-actor", "allowed")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transaction_api_composes_operations_and_rolls_back_as_one_unit() {
    let database = Store::memory().await.unwrap();
    let names = vec!["one".to_owned()];
    let mut tx = database.begin().await.unwrap();
    ToolStore::configure_from_host_in_transaction(
        &mut tx,
        CHAIN,
        ACTOR,
        acl(&["one"]),
        tools(&names),
    )
    .await
    .unwrap();
    let expanded = ToolStore::request_expansion_in_transaction(&mut tx, CHAIN, ACTOR, "one")
        .await
        .unwrap();
    assert_eq!(expanded.descriptor.name, "one");
    let execution = ToolStore::begin_execution_in_transaction(&mut tx, CHAIN, ACTOR, "one")
        .await
        .unwrap();
    assert_eq!(execution.lease.tool_name(), "one");
    tx.rollback().await.unwrap();

    let mut check = database.begin().await.unwrap();
    let missing = ToolStore::search_descriptions_in_transaction(&mut check, CHAIN, ACTOR, "")
        .await
        .unwrap_err();
    assert_eq!(missing.status, 404);
    check.rollback().await.unwrap();
}

#[tokio::test]
async fn reclaim_is_permission_checked_and_cannot_release_an_execution_lease() {
    let database = Store::memory().await.unwrap();
    let service = configured(database, &["tool"]).await;
    service
        .request_expansion(CHAIN, ACTOR, "tool")
        .await
        .unwrap();

    service
        .update_acl_from_host(
            CHAIN,
            ACTOR,
            ToolAcl::new(
                vec![],
                vec![AclGrant::new(ACTOR, "tool")],
                vec![AclGrant::new(ACTOR, "tool")],
            ),
        )
        .await
        .unwrap();
    let unauthorized = service
        .request_reclaim(CHAIN, ACTOR, "tool")
        .await
        .unwrap_err();
    assert_eq!(unauthorized.status, 403);

    service
        .update_acl_from_host(CHAIN, ACTOR, acl(&["tool"]))
        .await
        .unwrap();
    let running = service.begin_execution(CHAIN, ACTOR, "tool").await.unwrap();
    let refused = service
        .request_reclaim(CHAIN, ACTOR, "tool")
        .await
        .unwrap_err();
    assert_eq!(refused.status, 409);
    service
        .finish_execution(CHAIN, ACTOR, &running.lease)
        .await
        .unwrap();
    assert!(service.request_reclaim(CHAIN, ACTOR, "tool").await.unwrap());
    assert!(!service.request_reclaim(CHAIN, ACTOR, "tool").await.unwrap());
}

#[tokio::test]
async fn host_fixed_tool_cannot_be_reclaimed() {
    let database = Store::memory().await.unwrap();
    let service = ToolStore::new(database);
    service
        .configure_from_host(
            CHAIN,
            ACTOR,
            acl(&["fixed"]),
            vec![(descriptor("fixed"), true)],
        )
        .await
        .unwrap();
    service
        .request_expansion(CHAIN, ACTOR, "fixed")
        .await
        .unwrap();
    let refused = service
        .request_reclaim(CHAIN, ACTOR, "fixed")
        .await
        .unwrap_err();
    assert_eq!(refused.status, 409);
}

#[tokio::test]
async fn host_acl_revocation_applies_to_next_persisted_request() {
    let database = Store::memory().await.unwrap();
    let service = configured(database, &["tool"]).await;
    service
        .request_expansion(CHAIN, ACTOR, "tool")
        .await
        .unwrap();
    service
        .update_acl_from_host(CHAIN, ACTOR, ToolAcl::default())
        .await
        .unwrap();
    let error = service
        .begin_execution(CHAIN, ACTOR, "tool")
        .await
        .unwrap_err();
    assert_eq!(error.status, 403);
    let error = service
        .request_reclaim(CHAIN, ACTOR, "tool")
        .await
        .unwrap_err();
    assert_eq!(error.status, 403);
}

#[tokio::test]
async fn service_enforces_round_15_and_persists_deterministic_total_schema_lru() {
    let database = Store::memory().await.unwrap();
    let idle = configured(database.clone(), &["idle"]).await;
    idle.request_expansion(CHAIN, ACTOR, "idle").await.unwrap();
    for _ in 0..14 {
        assert!(
            idle.advance_worker_round(CHAIN, ACTOR)
                .await
                .unwrap()
                .is_empty()
        );
    }
    // Re-expansion is not actual use and does not reset the idle age.
    idle.request_expansion(CHAIN, ACTOR, "idle").await.unwrap();
    assert_eq!(
        idle.advance_worker_round(CHAIN, ACTOR).await.unwrap(),
        vec!["idle"]
    );

    let names: Vec<String> = (0..129).map(|index| format!("tool-{index:03}")).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let host_names: Vec<(ToolDescriptor, bool)> = tools(&names);
    let service = ToolStore::new(database.clone());
    service
        .configure_from_host("chain-tool-lru", ACTOR, acl(&name_refs), host_names)
        .await
        .unwrap();
    let dynamic_limit = 128 - prefix::common_tool_registry().len();
    for name in names.iter().take(dynamic_limit) {
        service
            .request_expansion("chain-tool-lru", ACTOR, name)
            .await
            .unwrap();
    }
    let last = service
        .request_expansion("chain-tool-lru", ACTOR, &names[dynamic_limit])
        .await
        .unwrap();
    assert_eq!(last.evicted, vec![names[0].clone()]);
    let mut tx = database.begin().await.unwrap();
    let expanded = ToolStore::expanded_descriptors_in_transaction(&mut tx, "chain-tool-lru", ACTOR)
        .await
        .unwrap();
    assert_eq!(expanded.len(), dynamic_limit);
    assert_eq!(expanded.len() + prefix::common_tool_registry().len(), 128);
    tx.commit().await.unwrap();
    let summaries = service
        .search_descriptions("chain-tool-lru", ACTOR, "tool-000")
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(!summaries[0].expanded);
}

#[tokio::test]
async fn process_reopen_recovers_active_lease_without_redispatch_and_keeps_it_pinned() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tools.sqlite3");
    let database = Store::create(&path).await.unwrap();
    let dynamic_limit = 128 - prefix::common_tool_registry().len();
    let names: Vec<String> = (0..dynamic_limit + 1)
        .map(|index| format!("restart-{index:03}"))
        .collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let service = ToolStore::new(database.clone());
    service
        .configure_from_host("chain-tool-restart", ACTOR, acl(&name_refs), tools(&names))
        .await
        .unwrap();
    let mut expected_leases = Vec::new();
    for name in names.iter().take(dynamic_limit) {
        expected_leases.push(
            service
                .begin_execution("chain-tool-restart", ACTOR, name)
                .await
                .unwrap()
                .lease,
        );
    }
    database.close().await;
    drop(service);
    drop(database);

    let reopened = Store::open(&path).await.unwrap();
    let service = ToolStore::new(reopened.clone());
    let active = service
        .active_leases("chain-tool-restart", ACTOR)
        .await
        .unwrap();
    assert_eq!(active, expected_leases);
    // 15 response completions do not reclaim an in-flight tool.
    for _ in 0..15 {
        service
            .advance_worker_round("chain-tool-restart", ACTOR)
            .await
            .unwrap();
    }
    let result = service
        .request_expansion("chain-tool-restart", ACTOR, &names[dynamic_limit])
        .await;
    assert!(result.is_err());
    service
        .finish_execution("chain-tool-restart", ACTOR, &active[0])
        .await
        .unwrap();
    // After lease reconciliation, capacity eviction can reclaim the least recent tool.
    let expansion = service
        .request_expansion("chain-tool-restart", ACTOR, &names[dynamic_limit])
        .await
        .unwrap();
    assert_eq!(expansion.evicted, vec![names[0].clone()]);
    reopened.close().await;
}

async fn persist_toolset(database: &Store, chain: &str, set: &ToolSet) {
    let serialized = set.to_json().unwrap();
    let digest = storage::sha256(serialized.as_bytes());
    let mut tx = database.begin().await.unwrap();
    ToolStore::initialize_in_transaction(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO sic_dynamic_toolsets(chain_id,actor,state,digest) VALUES(?,?,?,?)")
        .bind(chain)
        .bind(ACTOR)
        .bind(serialized)
        .bind(digest)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn worker_admission_migrates_old_limit_safely_and_completion_still_releases_pins() {
    let database = Store::memory().await.unwrap();
    let dynamic_limit = 128 - prefix::common_tool_registry().len();
    let names: Vec<String> = (0..dynamic_limit + 2)
        .map(|index| format!("legacy-{index:03}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut legacy = ToolSet::new(ACTOR, acl(&refs)); // Simulates persisted pre-budget state.
    for name in &names {
        legacy
            .register_tool(descriptor(name.clone()), false)
            .unwrap();
        legacy.request_expansion(name).unwrap();
    }
    persist_toolset(&database, "legacy-migrate", &legacy).await;

    let mut tx = database.begin().await.unwrap();
    let schemas = ToolStore::expanded_descriptors_in_transaction(&mut tx, "legacy-migrate", ACTOR)
        .await
        .unwrap();
    assert_eq!(schemas.len(), dynamic_limit);
    assert_eq!(schemas.len() + prefix::common_tool_registry().len(), 128);
    tx.commit().await.unwrap();

    let mut tx = database.begin().await.unwrap();
    let (state, digest): (String, String) =
        sqlx::query_as("SELECT state,digest FROM sic_dynamic_toolsets WHERE chain_id=?")
            .bind("legacy-migrate")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(storage::sha256(state.as_bytes()), digest);
    let migrated = ToolSet::from_json(&state).unwrap();
    assert_eq!(migrated.max_expanded(), dynamic_limit);
    assert_eq!(migrated.expanded_count(), dynamic_limit);
    tx.commit().await.unwrap();

    let next = ToolStore::new(database.clone())
        .request_expansion("legacy-migrate", ACTOR, &names[0])
        .await
        .unwrap();
    assert_eq!(next.evicted, vec![names[2].clone()]);

    let strict_names: Vec<String> = (0..5).map(|index| format!("strict-{index}")).collect();
    let strict_refs: Vec<&str> = strict_names.iter().map(String::as_str).collect();
    let mut stricter = ToolSet::with_limits(ACTOR, acl(&strict_refs), 15, 3);
    for name in &strict_names {
        stricter
            .register_tool(descriptor(name.clone()), false)
            .unwrap();
    }
    stricter.request_expansion(&strict_names[0]).unwrap();
    stricter.request_expansion(&strict_names[1]).unwrap();
    persist_toolset(&database, "legacy-strict", &stricter).await;
    let mut tx = database.begin().await.unwrap();
    let schemas = ToolStore::expanded_descriptors_in_transaction(&mut tx, "legacy-strict", ACTOR)
        .await
        .unwrap();
    assert_eq!(schemas.len(), 2);
    tx.commit().await.unwrap();
    let next = ToolStore::new(database.clone())
        .request_expansion("legacy-strict", ACTOR, &strict_names[2])
        .await
        .unwrap();
    assert!(next.evicted.is_empty());
    let mut tx = database.begin().await.unwrap();
    let (state,): (String,) =
        sqlx::query_as("SELECT state FROM sic_dynamic_toolsets WHERE chain_id=?")
            .bind("legacy-strict")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(ToolSet::from_json(&state).unwrap().max_expanded(), 3);
    tx.commit().await.unwrap();

    let mut historically_oversized = ToolSet::new(ACTOR, acl(&strict_refs));
    for name in &strict_names {
        historically_oversized
            .register_tool(descriptor(name.clone()), false)
            .unwrap();
        historically_oversized.request_expansion(name).unwrap();
    }
    let mut stored: Value =
        serde_json::from_str(&historically_oversized.to_json().unwrap()).unwrap();
    stored["max_expanded"] = json!(3);
    let historically_strict = ToolSet::from_json(&stored.to_string()).unwrap();
    persist_toolset(&database, "legacy-strict-overflow", &historically_strict).await;
    let mut tx = database.begin().await.unwrap();
    let schemas =
        ToolStore::expanded_descriptors_in_transaction(&mut tx, "legacy-strict-overflow", ACTOR)
            .await
            .unwrap();
    assert_eq!(schemas.len(), 3);
    tx.commit().await.unwrap();
    let mut tx = database.begin().await.unwrap();
    let (state,): (String,) =
        sqlx::query_as("SELECT state FROM sic_dynamic_toolsets WHERE chain_id=?")
            .bind("legacy-strict-overflow")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let migrated = ToolSet::from_json(&state).unwrap();
    assert_eq!(migrated.max_expanded(), 3);
    assert_eq!(migrated.expanded_count(), 3);
    tx.commit().await.unwrap();

    let pinned_names: Vec<String> = (0..dynamic_limit + 1)
        .map(|index| format!("pinned-{index:03}"))
        .collect();
    let pinned_refs: Vec<&str> = pinned_names.iter().map(String::as_str).collect();
    let mut pinned = ToolSet::new(ACTOR, acl(&pinned_refs));
    let mut leases = Vec::new();
    for name in &pinned_names {
        pinned
            .register_tool(descriptor(name.clone()), false)
            .unwrap();
        leases.push(pinned.begin_execution(name).unwrap().lease);
    }
    persist_toolset(&database, "legacy-pinned", &pinned).await;

    let mut tx = database.begin().await.unwrap();
    let rejected = ToolStore::expanded_descriptors_in_transaction(&mut tx, "legacy-pinned", ACTOR)
        .await
        .unwrap_err();
    assert_eq!(rejected.status, 409);
    drop(tx);

    // Lease settlement remains available while an old state is over budget.
    ToolStore::new(database.clone())
        .finish_execution("legacy-pinned", ACTOR, &leases[0])
        .await
        .unwrap();
    let mut tx = database.begin().await.unwrap();
    let schemas = ToolStore::expanded_descriptors_in_transaction(&mut tx, "legacy-pinned", ACTOR)
        .await
        .unwrap();
    assert_eq!(schemas.len(), dynamic_limit);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn host_fixed_catalog_cannot_push_complete_schema_count_over_128() {
    let database = Store::memory().await.unwrap();
    let limit = 128 - prefix::common_tool_registry().len();
    let names: Vec<String> = (0..limit + 1)
        .map(|index| format!("fixed-{index:03}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let attempted = names
        .iter()
        .map(|name| (descriptor(name.clone()), true))
        .collect();
    let result = ToolStore::new(database.clone())
        .configure_from_host("too-many-fixed", ACTOR, acl(&refs), attempted)
        .await;
    assert!(result.is_err());
    ToolStore::new(database.clone()).initialize().await.unwrap();
    let mut tx = database.begin().await.unwrap();
    let row: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sic_dynamic_toolsets WHERE chain_id=?")
            .bind("too-many-fixed")
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(row.is_none());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn catalog_sync_respects_global_budget_and_evicts_only_lru_dynamic_schema() {
    let database = Store::memory().await.unwrap();
    let dynamic_limit = 128 - prefix::common_tool_registry().len();
    let names: Vec<String> = (0..dynamic_limit + 1)
        .map(|index| format!("sync-{index:03}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let service = ToolStore::new(database.clone());
    service
        .configure_from_host(
            "sync-budget",
            ACTOR,
            acl(&refs),
            tools(&names[..dynamic_limit]),
        )
        .await
        .unwrap();
    for name in names.iter().take(dynamic_limit) {
        service
            .request_expansion("sync-budget", ACTOR, name)
            .await
            .unwrap();
    }

    let catalog = names
        .iter()
        .take(dynamic_limit)
        .map(|name| (descriptor(name.clone()), false))
        .chain(std::iter::once((
            descriptor(names[dynamic_limit].clone()),
            true,
        )))
        .collect();
    let mut tx = database.begin().await.unwrap();
    ToolStore::sync_from_host_in_transaction(&mut tx, "sync-budget", ACTOR, acl(&refs), catalog)
        .await
        .unwrap();
    let expanded = ToolStore::expanded_descriptors_in_transaction(&mut tx, "sync-budget", ACTOR)
        .await
        .unwrap();
    assert_eq!(expanded.len(), dynamic_limit);
    assert_eq!(expanded.len() + prefix::common_tool_registry().len(), 128);
    assert!(!expanded.iter().any(|tool| tool.name == names[0]));
    assert!(
        expanded
            .iter()
            .any(|tool| tool.name == names[dynamic_limit])
    );
    tx.commit().await.unwrap();
}
