//! Host-authorized dynamic tool discovery, expansion, and execution admission.
//!
//! Model-facing methods accept only search/expand/execute intent. The principal
//! and three independent ACLs are fixed by the host when constructing the set;
//! request text cannot grant permissions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DEFAULT_IDLE_ROUNDS: u64 = 15;
pub const DEFAULT_MAX_EXPANDED: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AclGrant {
    principal: String,
    tool_name: String,
}

impl AclGrant {
    /// Constructed by host configuration code, never from model request data.
    pub fn new(principal: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            principal: principal.into(),
            tool_name: tool_name.into(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolAcl {
    discover: Vec<AclGrant>,
    expand: Vec<AclGrant>,
    execute: Vec<AclGrant>,
}

impl ToolAcl {
    pub fn new(discover: Vec<AclGrant>, expand: Vec<AclGrant>, execute: Vec<AclGrant>) -> Self {
        Self {
            discover,
            expand,
            execute,
        }
    }

    fn allows(grants: &[AclGrant], principal: &str, tool_name: &str) -> bool {
        grants
            .iter()
            .any(|grant| grant.principal == principal && grant.tool_name == tool_name)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    pub expanded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct ToolState {
    descriptor: ToolDescriptor,
    expanded: bool,
    last_touched_round: Option<u64>,
    host_fixed: bool,
    in_flight: Vec<String>,
    #[serde(default = "default_available")]
    available: bool,
}

fn default_available() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionLease {
    id: String,
    tool_name: String,
}

impl ExecutionLease {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionStart {
    pub lease: ExecutionLease,
    pub descriptor: ToolDescriptor,
    pub evicted: Vec<String>,
}

/// Durable tool catalog. Keep mutation behind this API so model supplied text
/// cannot edit ACLs, pin state, the current round, or recency records.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolSet {
    principal: String,
    acl: ToolAcl,
    idle_rounds: u64,
    max_expanded: usize,
    current_worker_round: u64,
    next_lease: u64,
    tools: BTreeMap<String, ToolState>,
}

impl ToolSet {
    /// Build a standalone ToolSet with 15 idle Worker rounds and a 128-tool
    /// ceiling. ToolStore narrows that ceiling by the fixed shared-prefix
    /// registry size before persistence or Worker admission.
    pub fn new(principal: impl Into<String>, acl: ToolAcl) -> Self {
        Self::with_limits(principal, acl, DEFAULT_IDLE_ROUNDS, DEFAULT_MAX_EXPANDED)
    }

    /// Host-only policy setup for tests/deployments with an explicit limit.
    pub fn with_limits(
        principal: impl Into<String>,
        acl: ToolAcl,
        idle_rounds: u64,
        max_expanded: usize,
    ) -> Self {
        Self {
            principal: principal.into(),
            acl,
            idle_rounds,
            max_expanded,
            current_worker_round: 0,
            next_lease: 1,
            tools: BTreeMap::new(),
        }
    }

    /// Register a tool from host-owned configuration. Fixed tools start
    /// expanded and are permanently protected from eviction.
    pub fn register_tool(
        &mut self,
        descriptor: ToolDescriptor,
        host_fixed: bool,
    ) -> Result<(), String> {
        if descriptor.name.trim().is_empty() {
            return Err("tool name must not be empty".to_owned());
        }
        if !descriptor.schema.is_object() {
            return Err("tool schema must be an object".to_owned());
        }
        if self.tools.contains_key(&descriptor.name) {
            return Err("tool name is already registered".to_owned());
        }
        if host_fixed && self.expanded_count() >= self.max_expanded {
            self.make_room(&descriptor.name)?;
        }
        let name = descriptor.name.clone();
        self.tools.insert(
            name,
            ToolState {
                descriptor,
                expanded: host_fixed,
                last_touched_round: host_fixed.then_some(self.current_worker_round),
                host_fixed,
                in_flight: Vec::new(),
                available: true,
            },
        );
        Ok(())
    }

    /// Search discoverable tool descriptions. Schemas remain hidden until
    /// expansion; non-discoverable names are omitted.
    pub fn search_descriptions(&self, query: &str) -> Vec<ToolSummary> {
        let needle = query.to_lowercase();
        self.tools
            .iter()
            .filter(|(name, state)| {
                state.available
                    && ToolAcl::allows(&self.acl.discover, &self.principal, name)
                    && (needle.is_empty()
                        || name.to_lowercase().contains(&needle)
                        || state
                            .descriptor
                            .description
                            .to_lowercase()
                            .contains(&needle))
            })
            .map(|(_, state)| ToolSummary {
                name: state.descriptor.name.clone(),
                description: state.descriptor.description.clone(),
                expanded: state.expanded,
            })
            .collect()
    }

    /// Full schemas of currently expanded tools visible under current ACL.
    /// This directory is injected after the shared prefix, never in the
    /// common top-level registry.
    pub fn expanded_descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .iter()
            .filter(|(name, state)| {
                state.available
                    && state.expanded
                    && ToolAcl::allows(&self.acl.discover, &self.principal, name)
                    && ToolAcl::allows(&self.acl.expand, &self.principal, name)
            })
            .map(|(_, state)| state.descriptor.clone())
            .collect()
    }

    /// Handle a model or host request to expand a tool, subject to the
    /// independent host expansion ACL. This API never changes authorization.
    pub fn request_expansion(&mut self, tool_name: &str) -> Result<Expansion, String> {
        if !ToolAcl::allows(&self.acl.expand, &self.principal, tool_name) {
            return Err("tool expansion is not authorized".to_owned());
        }
        self.expand_authorized(tool_name)
    }

    /// Explicitly reclaim one discovered tool at the model's request. Both
    /// discovery and expansion permissions are required. Returning `false`
    /// means the tool was already unexpanded; pinned tools are rejected.
    pub fn request_reclaim(&mut self, tool_name: &str) -> Result<bool, String> {
        if !ToolAcl::allows(&self.acl.discover, &self.principal, tool_name)
            || !ToolAcl::allows(&self.acl.expand, &self.principal, tool_name)
        {
            return Err("tool reclaim is not authorized".to_owned());
        }
        let state = self
            .tools
            .get(tool_name)
            .ok_or_else(|| "unknown tool".to_owned())?;
        if !state.available {
            return Err("tool is absent from current host catalog".into());
        }
        if state.host_fixed {
            return Err("host-fixed tools cannot be reclaimed".to_owned());
        }
        if !state.in_flight.is_empty() {
            return Err("tools with in-flight execution cannot be reclaimed".to_owned());
        }
        if !state.expanded {
            return Ok(false);
        }
        self.tools
            .get_mut(tool_name)
            .expect("tool checked above")
            .expanded = false;
        Ok(true)
    }

    /// Admit execution only after rechecking the separate execute ACL. An
    /// unexpanded tool is expanded on demand, which also checks expand ACL.
    pub fn begin_execution(&mut self, tool_name: &str) -> Result<ExecutionStart, String> {
        if !ToolAcl::allows(&self.acl.execute, &self.principal, tool_name) {
            return Err("tool execution is not authorized".to_owned());
        }
        let state = self
            .tools
            .get(tool_name)
            .ok_or_else(|| "unknown tool".to_owned())?;
        if !state.available {
            return Err("tool is absent from current host catalog".into());
        }
        let expanded = state.expanded;
        let mut evicted = Vec::new();
        if !expanded {
            if !ToolAcl::allows(&self.acl.expand, &self.principal, tool_name) {
                return Err("execution requires separately authorized tool expansion".to_owned());
            }
            evicted = self.make_room(tool_name)?;
            let state = self
                .tools
                .get_mut(tool_name)
                .ok_or_else(|| "unknown tool".to_owned())?;
            state.expanded = true;
            state.last_touched_round = Some(self.current_worker_round);
        }
        // Actual execution, not another expansion request, constitutes use.
        self.tools
            .get_mut(tool_name)
            .expect("tool checked above")
            .last_touched_round = Some(self.current_worker_round);
        let id = format!("tool-lease-{}", self.next_lease);
        self.next_lease = self
            .next_lease
            .checked_add(1)
            .ok_or_else(|| "tool lease sequence exhausted".to_owned())?;
        let state = self
            .tools
            .get_mut(tool_name)
            .ok_or_else(|| "unknown tool".to_owned())?;
        state.in_flight.push(id.clone());
        let descriptor = state.descriptor.clone();
        Ok(ExecutionStart {
            lease: ExecutionLease {
                id,
                tool_name: tool_name.to_owned(),
            },
            descriptor,
            evicted,
        })
    }

    /// Finish an actual host execution. A completed attempt counts as recent
    /// use, regardless of its external result; unknown lease IDs are rejected.
    pub fn finish_execution(&mut self, lease: &ExecutionLease) -> Result<(), String> {
        let state = self
            .tools
            .get_mut(&lease.tool_name)
            .ok_or_else(|| "lease tool is not registered".to_owned())?;
        let index = state
            .in_flight
            .iter()
            .position(|id| id == &lease.id)
            .ok_or_else(|| "execution lease is not active".to_owned())?;
        state.in_flight.remove(index);
        state.last_touched_round = Some(self.current_worker_round);
        if !state.available && state.in_flight.is_empty() && !state.host_fixed {
            state.expanded = false;
        }
        Ok(())
    }

    /// Advance after one Worker response is complete and evict tools that
    /// have reached the configured idle age. Active and host-fixed tools pin.
    pub fn advance_worker_round(&mut self) -> Result<Vec<String>, String> {
        self.current_worker_round = self
            .current_worker_round
            .checked_add(1)
            .ok_or_else(|| "Worker round counter exhausted".to_owned())?;
        let mut candidates: Vec<(String, u64)> = self
            .tools
            .iter()
            .filter_map(|(name, state)| {
                if !state.expanded || state.host_fixed || !state.in_flight.is_empty() {
                    return None;
                }
                let last = state.last_touched_round.unwrap_or(0);
                (self.current_worker_round.saturating_sub(last) >= self.idle_rounds)
                    .then_some((name.clone(), last))
            })
            .collect();
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut evicted = Vec::new();
        for (name, _) in candidates {
            if self.remove_expanded(&name) {
                evicted.push(name);
            }
        }
        Ok(evicted)
    }

    pub fn current_worker_round(&self) -> u64 {
        self.current_worker_round
    }

    /// Replace authorization using host-owned policy. The next discovery,
    /// expansion, reclaim, or execution request observes the new grants.
    pub fn update_acl(&mut self, acl: ToolAcl) {
        self.acl = acl;
    }

    /// Reconcile a host-owned catalog. Removed or changed schemas lose their
    /// expansion; in-flight leases keep their original descriptor until they
    /// finish, and fixed tools cannot be silently removed or rewritten.
    pub fn sync_from_host(
        &mut self,
        acl: ToolAcl,
        tools: Vec<(ToolDescriptor, bool)>,
    ) -> Result<(), String> {
        let mut proposed = self.clone();
        let mut entries = BTreeMap::new();
        for (descriptor, fixed) in tools {
            if descriptor.name.trim().is_empty() || !descriptor.schema.is_object() {
                return Err("tool descriptor requires a name and object schema".into());
            }
            if entries
                .insert(descriptor.name.clone(), (descriptor, fixed))
                .is_some()
            {
                return Err("duplicate host tool descriptor".into());
            }
        }
        if acl
            .discover
            .iter()
            .chain(acl.expand.iter())
            .chain(acl.execute.iter())
            .any(|grant| !entries.contains_key(&grant.tool_name))
        {
            return Err("catalog ACL cannot grant a tool absent from the host catalog".into());
        }
        for (name, state) in &mut proposed.tools {
            match entries.get(name) {
                None => {
                    if state.host_fixed {
                        return Err("host-fixed tool cannot be removed by catalog sync".into());
                    }
                    if state.in_flight.is_empty() {
                        state.expanded = false;
                    }
                    state.available = false;
                }
                Some((descriptor, fixed)) => {
                    if state.host_fixed != *fixed {
                        return Err("catalog sync cannot change fixed-tool identity".into());
                    }
                    if state.descriptor != *descriptor {
                        if state.host_fixed || !state.in_flight.is_empty() {
                            return Err("cannot change fixed or in-flight tool schema".into());
                        }
                        state.descriptor = descriptor.clone();
                        state.expanded = false;
                        state.last_touched_round = None;
                    }
                    state.available = true;
                }
            }
            if !state.host_fixed
                && state.in_flight.is_empty()
                && (!ToolAcl::allows(&acl.discover, &proposed.principal, name)
                    || !ToolAcl::allows(&acl.expand, &proposed.principal, name))
            {
                state.expanded = false;
            }
        }
        for (name, (descriptor, fixed)) in entries {
            if !proposed.tools.contains_key(&name) {
                proposed.register_tool(descriptor, fixed)?;
            }
        }
        proposed.acl = acl;
        *self = proposed;
        Ok(())
    }

    pub fn expanded_count(&self) -> usize {
        self.tools.values().filter(|state| state.expanded).count()
    }

    pub fn max_expanded(&self) -> usize {
        self.max_expanded
    }

    /// Tighten the persisted dynamic-schema ceiling. It never evicts
    /// host-fixed or in-flight tools and is atomic on failure.
    pub fn constrain_expanded_limit(&mut self, limit: usize) -> Result<Vec<String>, String> {
        let mut proposed = self.clone();
        proposed.max_expanded = proposed.max_expanded.min(limit);
        let mut evicted = Vec::new();
        while proposed.expanded_count() > proposed.max_expanded {
            let victim = proposed
                .tools
                .iter()
                .filter(|(_, state)| {
                    state.expanded && !state.host_fixed && state.in_flight.is_empty()
                })
                .map(|(name, state)| (name.clone(), state.last_touched_round.unwrap_or(0)))
                .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                .map(|(name, _)| name)
                .ok_or_else(|| {
                    "expanded-tool limit exceeded; fixed or in-flight tools cannot be reclaimed"
                        .to_owned()
                })?;
            proposed.remove_expanded(&victim);
            evicted.push(victim);
        }
        *self = proposed;
        Ok(evicted)
    }

    pub fn is_expanded(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .is_some_and(|state| state.expanded)
    }

    /// Serialize the complete catalog, ACLs, recency, active leases, and
    /// counters. In-flight executions remain pinned after restore until the
    /// host explicitly finishes or reconciles their leases.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("toolset serialization failed: {error}"))
    }

    pub fn from_json(serialized: &str) -> Result<Self, String> {
        serde_json::from_str(serialized)
            .map_err(|error| format!("toolset deserialization failed: {error}"))
    }

    fn expand_authorized(&mut self, tool_name: &str) -> Result<Expansion, String> {
        let state = self
            .tools
            .get(tool_name)
            .ok_or_else(|| "unknown tool".to_owned())?;
        if !state.available {
            return Err("tool is absent from current host catalog".into());
        }
        let already_expanded = state.expanded;
        let evicted = if already_expanded {
            Vec::new()
        } else {
            self.make_room(tool_name)?
        };
        let state = self
            .tools
            .get_mut(tool_name)
            .ok_or_else(|| "unknown tool".to_owned())?;
        state.expanded = true;
        if !already_expanded {
            state.last_touched_round = Some(self.current_worker_round);
        }
        Ok(Expansion {
            descriptor: state.descriptor.clone(),
            evicted,
        })
    }

    fn make_room(&mut self, target: &str) -> Result<Vec<String>, String> {
        let mut evicted = Vec::new();
        if self.max_expanded == 0 {
            return Err("expanded-tool limit is zero".to_owned());
        }
        while self.expanded_count() >= self.max_expanded {
            let victim = self
                .tools
                .iter()
                .filter(|(name, state)| {
                    name.as_str() != target
                        && state.expanded
                        && !state.host_fixed
                        && state.in_flight.is_empty()
                })
                .map(|(name, state)| (name.clone(), state.last_touched_round.unwrap_or(0)))
                .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                .map(|(name, _)| name);
            let Some(victim) = victim else {
                return Err(
                    "expanded-tool limit reached; no authorized eviction candidate".to_owned(),
                );
            };
            self.remove_expanded(&victim);
            evicted.push(victim);
        }
        Ok(evicted)
    }

    fn remove_expanded(&mut self, name: &str) -> bool {
        if let Some(state) = self.tools.get_mut(name) {
            if state.expanded && !state.host_fixed && state.in_flight.is_empty() {
                state.expanded = false;
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Expansion {
    pub descriptor: ToolDescriptor,
    pub evicted: Vec<String>,
}
