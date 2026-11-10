#[path = "../src/toolset.rs"]
mod toolset;

use serde_json::json;
use toolset::{
    AclGrant, DEFAULT_IDLE_ROUNDS, DEFAULT_MAX_EXPANDED, ToolAcl, ToolDescriptor, ToolSet,
};

fn grant(names: &[&str]) -> Vec<AclGrant> {
    names
        .iter()
        .map(|name| AclGrant::new("worker-A", *name))
        .collect()
}

fn acl(discover: &[&str], expand: &[&str], execute: &[&str]) -> ToolAcl {
    ToolAcl::new(grant(discover), grant(expand), grant(execute))
}

fn descriptor(name: impl Into<String>) -> ToolDescriptor {
    let name = name.into();
    ToolDescriptor {
        description: format!("Searchable description for {name}"),
        schema: json!({
            "type":"function",
            "function":{"name":name,"parameters":{"type":"object","properties":{}}}
        }),
        name,
    }
}

#[test]
fn standalone_defaults_are_fifteen_idle_rounds_and_128_expanded_tools() {
    assert_eq!(DEFAULT_IDLE_ROUNDS, 15);
    assert_eq!(DEFAULT_MAX_EXPANDED, 128);
}

#[test]
fn host_catalog_sync_preserves_inflight_leases_and_revokes_removed_tools() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &["a"]));
    set.register_tool(descriptor("a"), false).unwrap();
    let lease = set.begin_execution("a").unwrap().lease;
    let before = set.to_json().unwrap();
    let mut changed = descriptor("a");
    changed.description = "new schema version".into();
    assert!(
        set.sync_from_host(acl(&["a"], &["a"], &["a"]), vec![(changed.clone(), false)])
            .is_err()
    );
    assert_eq!(set.to_json().unwrap(), before);
    set.sync_from_host(acl(&["b"], &["b"], &["b"]), vec![(descriptor("b"), false)])
        .unwrap();
    assert!(set.begin_execution("a").is_err());
    assert!(set.is_expanded("a"), "active lease remains pinned");
    set.finish_execution(&lease).unwrap();
    set.sync_from_host(acl(&["b"], &["b"], &["b"]), vec![(descriptor("b"), false)])
        .unwrap();
    assert!(!set.is_expanded("a"));
    assert_eq!(set.search_descriptions("").len(), 1);
    assert_eq!(set.search_descriptions("")[0].name, "b");
    assert!(set.begin_execution("b").is_ok());
}

#[test]
fn idle_tool_survives_round_14_and_is_evicted_at_round_15() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &["a"]));
    set.register_tool(descriptor("a"), false).unwrap();
    set.request_expansion("a").unwrap();
    for _ in 0..14 {
        assert!(set.advance_worker_round().unwrap().is_empty());
    }
    assert!(set.is_expanded("a"));
    assert_eq!(set.current_worker_round(), 14);
    assert_eq!(set.advance_worker_round().unwrap(), vec!["a"]);
    assert!(!set.is_expanded("a"));
}

#[test]
fn repeated_expansion_does_not_reset_unused_age() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &["a"]));
    set.register_tool(descriptor("a"), false).unwrap();
    set.request_expansion("a").unwrap();
    for _ in 0..10 {
        set.advance_worker_round().unwrap();
    }
    set.request_expansion("a").unwrap();
    for _ in 0..4 {
        assert!(set.advance_worker_round().unwrap().is_empty());
    }
    assert!(set.is_expanded("a"));
    assert_eq!(set.advance_worker_round().unwrap(), vec!["a"]);
}

#[test]
fn actual_execution_start_resets_unused_age() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &["a"]));
    set.register_tool(descriptor("a"), false).unwrap();
    set.request_expansion("a").unwrap();
    for _ in 0..10 {
        set.advance_worker_round().unwrap();
    }
    let lease = set.begin_execution("a").unwrap().lease;
    set.finish_execution(&lease).unwrap();
    for _ in 0..14 {
        assert!(set.advance_worker_round().unwrap().is_empty());
    }
    assert!(set.is_expanded("a"));
    assert_eq!(set.advance_worker_round().unwrap(), vec!["a"]);
}

#[test]
fn expanding_past_128_evicts_deterministic_least_recent_tool() {
    let names: Vec<String> = (0..129).map(|i| format!("tool-{i:03}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut set = ToolSet::new("worker-A", acl(&refs, &refs, &refs));
    for name in &names {
        set.register_tool(descriptor(name.clone()), false).unwrap();
    }
    for name in names.iter().take(128) {
        set.request_expansion(name).unwrap();
    }
    assert_eq!(set.expanded_count(), 128);
    let result = set.request_expansion(&names[128]).unwrap();
    assert_eq!(result.evicted, vec![names[0].clone()]);
    assert!(!set.is_expanded(&names[0]));
    assert!(set.is_expanded(&names[128]));
    assert_eq!(set.expanded_count(), 128);
}

#[test]
fn discovery_acl_is_independent_from_expand_and_execute() {
    let mut set = ToolSet::new(
        "worker-A",
        acl(&["visible"], &["hidden", "visible"], &["visible"]),
    );
    set.register_tool(descriptor("visible"), false).unwrap();
    set.register_tool(descriptor("hidden"), false).unwrap();
    let found = set.search_descriptions("searchable");
    assert_eq!(
        found.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
        vec!["visible"]
    );
    assert!(set.request_expansion("hidden").is_ok());
    assert!(
        !set.search_descriptions("hidden")
            .iter()
            .any(|x| x.name == "hidden")
    );
}

#[test]
fn execute_acl_is_rechecked_even_after_discovery_and_expansion() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &[]));
    set.register_tool(descriptor("a"), false).unwrap();
    assert_eq!(set.search_descriptions("a").len(), 1);
    set.request_expansion("a").unwrap();
    assert!(
        set.begin_execution("a")
            .unwrap_err()
            .contains("not authorized")
    );
    assert!(set.is_expanded("a"));
}

#[test]
fn host_acl_update_is_observed_by_the_next_request() {
    let mut set = ToolSet::new("worker-A", acl(&["a"], &["a"], &["a"]));
    set.register_tool(descriptor("a"), false).unwrap();
    set.request_expansion("a").unwrap();
    set.update_acl(acl(&[], &[], &[]));
    assert!(set.search_descriptions("a").is_empty());
    assert!(
        set.request_expansion("a")
            .unwrap_err()
            .contains("not authorized")
    );
    assert!(
        set.begin_execution("a")
            .unwrap_err()
            .contains("not authorized")
    );
}

#[test]
fn explicit_reclaim_only_reclaims_the_named_permitted_tool() {
    let mut set = ToolSet::new("worker-A", acl(&["a", "b"], &["a", "b"], &["a", "b"]));
    set.register_tool(descriptor("a"), false).unwrap();
    set.register_tool(descriptor("b"), false).unwrap();
    set.request_expansion("a").unwrap();
    set.request_expansion("b").unwrap();
    assert!(set.request_reclaim("a").unwrap());
    assert!(!set.request_reclaim("a").unwrap());
    assert!(!set.is_expanded("a"));
    assert!(set.is_expanded("b"));
}

#[test]
fn explicit_reclaim_requires_both_discovery_and_expansion_acl() {
    let mut cannot_discover = ToolSet::new("worker-A", acl(&[], &["a"], &["a"]));
    cannot_discover
        .register_tool(descriptor("a"), false)
        .unwrap();
    cannot_discover.request_expansion("a").unwrap();
    assert!(
        cannot_discover
            .request_reclaim("a")
            .unwrap_err()
            .contains("not authorized")
    );

    let mut cannot_expand = ToolSet::new("worker-A", acl(&["a"], &[], &["a"]));
    cannot_expand.register_tool(descriptor("a"), true).unwrap();
    assert!(
        cannot_expand
            .request_reclaim("a")
            .unwrap_err()
            .contains("not authorized")
    );
}

#[test]
fn explicit_reclaim_rejects_in_flight_and_host_fixed_tools() {
    let mut set = ToolSet::new(
        "worker-A",
        acl(&["a", "fixed"], &["a", "fixed"], &["a", "fixed"]),
    );
    set.register_tool(descriptor("a"), false).unwrap();
    set.register_tool(descriptor("fixed"), true).unwrap();
    set.request_expansion("a").unwrap();
    let lease = set.begin_execution("a").unwrap().lease;
    assert!(set.request_reclaim("a").unwrap_err().contains("in-flight"));
    assert!(
        set.request_reclaim("fixed")
            .unwrap_err()
            .contains("host-fixed")
    );
    set.finish_execution(&lease).unwrap();
    assert!(set.request_reclaim("a").unwrap());
}

#[test]
fn execution_on_demand_requires_both_expand_and_execute_acl() {
    let mut no_expand = ToolSet::new("worker-A", acl(&["a"], &[], &["a"]));
    no_expand.register_tool(descriptor("a"), false).unwrap();
    assert!(
        no_expand
            .begin_execution("a")
            .unwrap_err()
            .contains("separately authorized")
    );
    assert!(!no_expand.is_expanded("a"));

    let mut allowed = ToolSet::new("worker-A", acl(&[], &["a"], &["a"]));
    allowed.register_tool(descriptor("a"), false).unwrap();
    let started = allowed.begin_execution("a").unwrap();
    assert!(allowed.is_expanded("a"));
    assert_eq!(started.descriptor.name, "a");
    allowed.finish_execution(&started.lease).unwrap();
}

#[test]
fn in_flight_and_host_fixed_tools_are_not_reclaimed_and_full_pinned_set_refuses() {
    let mut set = ToolSet::with_limits(
        "worker-A",
        acl(&["a", "b"], &["a", "b"], &["a", "b"]),
        15,
        1,
    );
    set.register_tool(descriptor("a"), false).unwrap();
    set.register_tool(descriptor("b"), false).unwrap();
    set.request_expansion("a").unwrap();
    let lease = set.begin_execution("a").unwrap().lease;
    assert_eq!(lease.tool_name(), "a");
    assert!(!lease.id().is_empty());
    assert!(
        set.request_expansion("b")
            .unwrap_err()
            .contains("no authorized eviction")
    );
    assert!(set.is_expanded("a"));
    set.finish_execution(&lease).unwrap();
    assert_eq!(set.request_expansion("b").unwrap().evicted, vec!["a"]);

    let mut fixed = ToolSet::with_limits(
        "worker-A",
        acl(
            &["fixed", "dynamic"],
            &["fixed", "dynamic"],
            &["fixed", "dynamic"],
        ),
        15,
        1,
    );
    fixed.register_tool(descriptor("fixed"), true).unwrap();
    fixed.register_tool(descriptor("dynamic"), false).unwrap();
    assert!(
        fixed
            .request_expansion("dynamic")
            .unwrap_err()
            .contains("no authorized eviction")
    );
    assert!(fixed.is_expanded("fixed"));
}

#[test]
fn idle_cleanup_skips_in_flight_and_fixed_tools() {
    let mut set = ToolSet::new(
        "worker-A",
        acl(&["a", "fixed"], &["a", "fixed"], &["a", "fixed"]),
    );
    set.register_tool(descriptor("a"), false).unwrap();
    set.register_tool(descriptor("fixed"), true).unwrap();
    set.request_expansion("a").unwrap();
    let lease = set.begin_execution("a").unwrap().lease;
    for _ in 0..15 {
        set.advance_worker_round().unwrap();
    }
    assert!(set.is_expanded("a"));
    assert!(set.is_expanded("fixed"));
    set.finish_execution(&lease).unwrap();
    for _ in 0..15 {
        set.advance_worker_round().unwrap();
    }
    assert!(!set.is_expanded("a"));
    assert!(set.is_expanded("fixed"));
}

#[test]
fn serialized_state_roundtrip_preserves_acl_recency_and_active_lease() {
    let mut set = ToolSet::with_limits(
        "worker-A",
        acl(&["a", "b"], &["a", "b"], &["a", "b"]),
        15,
        1,
    );
    set.register_tool(descriptor("a"), false).unwrap();
    set.register_tool(descriptor("b"), false).unwrap();
    set.request_expansion("a").unwrap();
    let lease = set.begin_execution("a").unwrap().lease;
    assert_eq!(lease.tool_name(), "a");
    assert!(!lease.id().is_empty());
    let serialized = set.to_json().unwrap();
    let mut restored = ToolSet::from_json(&serialized).unwrap();
    assert_eq!(restored.to_json().unwrap(), serialized);
    for _ in 0..15 {
        restored.advance_worker_round().unwrap();
    }
    assert!(restored.is_expanded("a"));
    assert!(
        restored
            .request_expansion("b")
            .unwrap_err()
            .contains("no authorized eviction")
    );
    restored.finish_execution(&lease).unwrap();
    assert_eq!(restored.request_expansion("b").unwrap().evicted, vec!["a"]);
}
