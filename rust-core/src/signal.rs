//! Signal control plane: one lifecycle authority over Agent routes.
//!
//! Signal generalises a single Agent self-control to multi-Agent control. It
//! unifies Spawn/Fork, Pause, Stop, source admission and delegated control
//! rights behind one durable, timestamped, auditable state machine.
//!
//! This module is pure data plus synchronous transitions. It never starts,
//! pauses or kills an OS process and never treats model text as authority:
//! the host binds observing this state to obeying this state.

use crate::{
    error::{CoreError, Result},
    ipc::Principal,
    storage::{self, Store},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn s_err(message: impl Into<String>) -> String {
    message.into()
}

fn nonempty(value: &str, name: &str) -> std::result::Result<(), String> {
    if value.trim().is_empty() {
        Err(s_err(format!("{name} must be nonempty")))
    } else {
        Ok(())
    }
}

/// Rights over another Agent. An Agent always holds all of them on itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRight {
    Pause,
    Stop,
    Spawn,
    Wake,
    SourcePolicy,
    Grant,
}

impl ControlRight {
    pub fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "pause" => Ok(Self::Pause),
            "stop" => Ok(Self::Stop),
            "spawn" => Ok(Self::Spawn),
            "wake" => Ok(Self::Wake),
            "source_policy" => Ok(Self::SourcePolicy),
            "grant" => Ok(Self::Grant),
            other => Err(s_err(format!("unknown control right: {other}"))),
        }
    }
    pub fn all() -> BTreeSet<Self> {
        [Self::Pause, Self::Stop, Self::Spawn, Self::Wake, Self::SourcePolicy, Self::Grant]
            .into_iter()
            .collect()
    }
}

/// One durable delegation of rights from one Agent to another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlGrant {
    pub holder_route_id: String,
    pub target_route_id: String,
    pub rights: BTreeSet<ControlRight>,
    pub granted_by_route_id: String,
    pub granted_at: u64,
    pub revision: u64,
}

/// Where a child came from. An empty CB prefix is a Spawn, not a fork.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForkOrigin {
    Spawned,
    Forked {
        origin_route_id: String,
        prefix_cb_ids: Vec<String>,
        origin_cut_id: Option<String>,
        self_fork: bool,
    },
}

impl ForkOrigin {
    pub fn origin_route(&self) -> Option<&str> {
        match self {
            Self::Spawned => None,
            Self::Forked { origin_route_id, .. } => Some(origin_route_id),
        }
    }
    pub fn is_spawn(&self) -> bool {
        matches!(self, Self::Spawned)
    }
}

/// Categories every admissible information source is registered under. An
/// unregistered source can never be admitted and can never wake an Agent,
/// which keeps the allowed-wake-source set total rather than partial.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    ChildReply,
    AgentMessage,
    BackgroundTask,
    AdminSignal,
    ClockTimeout,
    HostEvent,
}

impl SourceKind {
    pub fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "child_reply" => Ok(Self::ChildReply),
            "agent_message" => Ok(Self::AgentMessage),
            "background_task" => Ok(Self::BackgroundTask),
            "admin_signal" => Ok(Self::AdminSignal),
            "clock_timeout" => Ok(Self::ClockTimeout),
            "host_event" => Ok(Self::HostEvent),
            other => Err(s_err(format!("unknown source kind: {other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRegistration {
    pub source_id: String,
    pub kind: SourceKind,
    pub owner: Option<String>,
    pub registered_by_route_id: String,
    pub registered_at: u64,
}

/// Which registered sources may reach an Agent while it is suspended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePolicy {
    pub kinds: BTreeSet<SourceKind>,
    pub source_ids: BTreeSet<String>,
    pub updated_at: u64,
    pub revision: u64,
}

impl SourcePolicy {
    pub fn admits(&self, source: &SourceRegistration) -> bool {
        self.kinds.contains(&source.kind) || self.source_ids.contains(&source.source_id)
    }
    pub fn is_open(&self) -> bool {
        !self.kinds.is_empty() || !self.source_ids.is_empty()
    }
}

/// Whether a Signal takes effect immediately or at the output boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    AfterOutput,
    Now,
}

impl ApplyMode {
    pub fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "after_output" => Ok(Self::AfterOutput),
            "now" => Ok(Self::Now),
            other => Err(s_err(format!("unknown apply mode: {other}"))),
        }
    }
}

/// The Agent lifecycle state under Signal control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ControlState {
    Running,
    PausePending { since: u64 },
    Paused { since: u64, wake_reason: Option<String> },
    StopPending { since: u64 },
    Stopped { since: u64, terminal: bool },
}

impl ControlState {
    pub fn name(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::PausePending { .. } => "pause_pending",
            Self::Paused { .. } => "paused",
            Self::StopPending { .. } => "stop_pending",
            Self::Stopped { .. } => "stopped",
        }
    }
    pub fn accepts_input(&self) -> bool {
        match self {
            Self::Running | Self::PausePending { .. } | Self::StopPending { .. } => true,
            Self::Paused { .. } | Self::Stopped { .. } => false,
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped { terminal: true, .. })
    }
    pub fn since(&self) -> u64 {
        match self {
            Self::Running => 0,
            Self::PausePending { since }
            | Self::StopPending { since }
            | Self::Paused { since, .. }
            | Self::Stopped { since, .. } => *since,
        }
    }
}

/// A managed arrival that waits for admission or for a resumed Agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedItem {
    pub seq: u64,
    pub received_at: u64,
    pub source_id: String,
    pub kind: SourceKind,
    pub payload: Value,
    pub admitted_at: Option<u64>,
    pub wake_eligible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    pub seq: u64,
    pub at: u64,
    pub action: String,
    pub actor_id: String,
    pub actor_route_id: String,
    pub target_route_id: Option<String>,
    pub detail: Value,
}

/// Scale bounds on delegated creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalLimits {
    pub max_child_depth: u32,
    pub max_owned_children: u32,
}

impl Default for SignalLimits {
    fn default() -> Self {
        Self { max_child_depth: 8, max_owned_children: 8 }
    }
}

impl SignalLimits {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_child_depth > 64 {
            return Err(s_err("max_child_depth must be 0..64"));
        }
        if self.max_owned_children > 256 {
            return Err(s_err("max_owned_children must be 0..256"));
        }
        Ok(())
    }
}

/// One Agent node in the Signal graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentNode {
    pub route_id: String,
    pub parents: Vec<String>,
    pub depth: u32,
    pub origin: ForkOrigin,
    pub children: Vec<String>,
    pub state: ControlState,
    pub policy: SourcePolicy,
    pub limits: SignalLimits,
    pub created_at: u64,
    pub control_epoch: u64,
    pub queue: Vec<QueuedItem>,
    pub next_seq: u64,
}

impl AgentNode {
    fn allocate(&mut self) -> u64 {
        let value = self.next_seq;
        self.next_seq += 1;
        value
    }
    pub fn pending(&self) -> usize {
        self.queue.iter().filter(|item| item.admitted_at.is_none()).count()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalState {
    nodes: BTreeMap<String, AgentNode>,
    sources: BTreeMap<String, SourceRegistration>,
    grants: BTreeMap<String, BTreeMap<String, ControlGrant>>,
    log: Vec<LogEntry>,
    next_log_seq: u64,
    next_global_seq: u64,
}

/// Everything needed to create a child. A missing origin means Spawn from an
/// empty CB prefix; a named origin plus prefix CBs means Fork.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnSpec {
    pub child_route_id: String,
    #[serde(default)]
    pub co_parents: Vec<String>,
    #[serde(default)]
    pub origin_route_id: Option<String>,
    #[serde(default)]
    pub prefix_cb_ids: Vec<String>,
    #[serde(default)]
    pub origin_cut_id: Option<String>,
    #[serde(default)]
    pub limits: Option<SignalLimits>,
    #[serde(default)]
    pub initial_policy: Option<SourcePolicy>,
}

impl SignalState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn node(&self, route_id: &str) -> Option<&AgentNode> {
        self.nodes.get(route_id)
    }
    pub fn nodes(&self) -> &BTreeMap<String, AgentNode> {
        &self.nodes
    }
    pub fn source(&self, source_id: &str) -> Option<&SourceRegistration> {
        self.sources.get(source_id)
    }

    fn log(
        &mut self,
        action: &str,
        principal: &Principal,
        target: Option<&str>,
        detail: Value,
    ) -> u64 {
        let seq = self.next_log_seq;
        self.next_log_seq += 1;
        self.log.push(LogEntry {
            seq,
            at: now(),
            action: action.to_owned(),
            actor_id: principal.actor_id.clone(),
            actor_route_id: principal.route_id.clone(),
            target_route_id: target.map(str::to_owned),
            detail,
        });
        if self.log.len() > 256 {
            let excess = self.log.len() - 256;
            self.log.drain(0..excess);
        }
        seq
    }

    /// Rights held by the principal over the target. Self-relation is full.
    pub fn held(&self, principal: &Principal, target: &str) -> BTreeSet<ControlRight> {
        if principal.route_id == target {
            return ControlRight::all();
        }
        self.grants
            .get(&principal.route_id)
            .and_then(|by_target| by_target.get(target))
            .map(|grant| grant.rights.clone())
            .unwrap_or_default()
    }

    fn require(
        &self,
        principal: &Principal,
        target: &str,
        right: ControlRight,
    ) -> std::result::Result<(), String> {
        if self.held(principal, target).contains(&right) {
            return Ok(());
        }
        Err(s_err("the required control right is not held"))
    }

    /// Register an information source. Nothing may arrive unregistered.
    pub fn register_source(
        &mut self,
        principal: &Principal,
        source_id: &str,
        kind: SourceKind,
        owner: Option<String>,
    ) -> Result<Value> {
        nonempty(source_id, "source identity").map_err(CoreError::invalid)?;
        if let Some(existing) = self.sources.get(source_id) {
            if existing.kind == kind && existing.owner == owner {
                return Ok(json!({"source_id":source_id,"kind":kind,"replayed":true}));
            }
            return Err(CoreError::conflict(
                "source identity reused with a different kind or owner",
            ));
        }
        let sequence = self.next_global_seq;
        self.next_global_seq += 1;
        let registration = SourceRegistration {
            source_id: source_id.to_owned(),
            kind: kind.clone(),
            owner,
            registered_by_route_id: principal.route_id.clone(),
            registered_at: now(),
        };
        self.sources.insert(source_id.to_owned(), registration);
        self.log(
            "source.registered",
            principal,
            None,
            json!({"source_id":source_id,"kind":kind,"seq":sequence}),
        );
        Ok(json!({"source_id":source_id,"kind":kind,"replayed":false}))
    }

    /// Publish the set of sources admitted while this Agent is suspended.
    pub fn set_source_policy(
        &mut self,
        principal: &Principal,
        target: &str,
        kinds: BTreeSet<SourceKind>,
        source_ids: BTreeSet<String>,
    ) -> Result<Value> {
        self.require(principal, target, ControlRight::SourcePolicy)
            .map_err(CoreError::forbidden)?;
        for source_id in &source_ids {
            if !self.sources.contains_key(source_id) {
                return Err(CoreError::invalid(
                    "a source must be registered before it can be admitted",
                ));
            }
        }
        let node = self
            .nodes
            .get_mut(target)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        if node.state.is_terminal() {
            return Err(CoreError::conflict("a terminal Agent accepts no sources"));
        }
        let revision = node.policy.revision + 1;
        node.policy = SourcePolicy { kinds, source_ids, updated_at: now(), revision };
        let policy = node.policy.clone();
        self.log(
            "signal.source_policy",
            principal,
            Some(target),
            json!({"revision":revision}),
        );
        Ok(json!({"target_route_id":target,"policy":policy}))
    }

    /// Signal-Spawn / Signal-Fork. An empty CB prefix is a Spawn; a named
    /// prefix of an existing route is a Fork, including a self-fork.
    pub fn spawn(&mut self, principal: &Principal, spec: SpawnSpec) -> Result<Value> {
        nonempty(&spec.child_route_id, "child route identity").map_err(CoreError::invalid)?;
        if self.nodes.contains_key(&spec.child_route_id) {
            return Err(CoreError::conflict("child route identity already exists"));
        }
        let origin = match spec.origin_route_id.as_deref() {
            None => {
                if !spec.prefix_cb_ids.is_empty() {
                    return Err(CoreError::invalid(
                        "a CB prefix requires an origin route; an empty prefix is a Spawn",
                    ));
                }
                ForkOrigin::Spawned
            }
            Some(origin_route_id) => {
                if spec.prefix_cb_ids.is_empty() {
                    return Err(CoreError::invalid(
                        "a fork requires at least one CB in the inherited prefix",
                    ));
                }
                let origin_node = self.nodes.get(origin_route_id).ok_or_else(|| {
                    CoreError::invalid("origin route is not under Signal control")
                })?;
                let depth = origin_node.depth + 1;
                let limits = origin_node.limits.clone();
                if depth > limits.max_child_depth {
                    return Err(CoreError::new(
                        "signal_depth_limit",
                        "the fork would exceed the origin Agent maximum child depth",
                        409,
                    ));
                }
                if origin_node.children.len() as u32 >= limits.max_owned_children {
                    return Err(CoreError::new(
                        "signal_children_limit",
                        "the origin Agent already holds its maximum number of children",
                        409,
                    ));
                }
                ForkOrigin::Forked {
                    origin_route_id: origin_route_id.to_owned(),
                    prefix_cb_ids: spec.prefix_cb_ids.clone(),
                    origin_cut_id: spec.origin_cut_id.clone(),
                    self_fork: origin_route_id == principal.route_id,
                }
            }
        };
        let origin_route = origin.origin_route().map(str::to_owned);
        // A fresh Spawn also consumes the requester owns-children budget.
        if origin.is_spawn() {
            if let Some(parent) = self.nodes.get(&principal.route_id) {
                if parent.children.len() as u32 >= parent.limits.max_owned_children {
                    return Err(CoreError::new(
                        "signal_children_limit",
                        "the requesting Agent already holds its maximum number of children",
                        409,
                    ));
                }
            }
        }
        let authorize = origin_route.clone().unwrap_or_else(|| principal.route_id.clone());
        self.require(principal, &authorize, ControlRight::Spawn)
            .map_err(CoreError::forbidden)?;
        let depth = match &origin {
            ForkOrigin::Spawned => 0,
            ForkOrigin::Forked { origin_route_id, .. } => self.nodes[origin_route_id].depth + 1,
        };
        let mut parents = if spec.child_route_id == principal.route_id && origin.is_spawn() {
            // An Agent registering itself as a fresh root has no parent.
            Vec::new()
        } else {
            let mut seeded = Vec::new();
            if let Some(route) = origin.origin_route() {
                seeded.push(route.to_owned());
            }
            if seeded.is_empty() || principal.route_id != seeded[0] {
                seeded.insert(0, principal.route_id.clone());
            }
            seeded
        };
        for parent in spec.co_parents.iter() {
            if parent == &principal.route_id {
                continue;
            }
            let node = self.nodes.get(parent).ok_or_else(|| {
                CoreError::invalid("a co-parent is not under Signal control")
            })?;
            if node.state.is_terminal() {
                return Err(CoreError::conflict("a co-parent is terminated"));
            }
            if !parents.contains(parent) {
                parents.push(parent.clone());
            }
        }
        let inherited_limits = match spec.limits.clone() {
            Some(limits) => limits,
            None => match &origin_route {
                Some(route) => self.nodes[route].limits.clone(),
                None => SignalLimits::default(),
            },
        };
        inherited_limits.validate().map_err(CoreError::invalid)?;
        let mut policy = spec.initial_policy.unwrap_or_default();
        policy.revision = 1;
        policy.updated_at = now();
        let mut node = AgentNode {
            route_id: spec.child_route_id.clone(),
            parents: parents.clone(),
            depth,
            origin: origin.clone(),
            children: Vec::new(),
            state: ControlState::Running,
            policy,
            limits: inherited_limits,
            created_at: now(),
            control_epoch: 0,
            queue: Vec::new(),
            next_seq: 1,
        };
        node.allocate();
        self.nodes.insert(spec.child_route_id.clone(), node);
        for parent in &parents {
            self.insert_grant(parent, &spec.child_route_id, ControlRight::all(), parent);
        }
        for parent in &parents {
            if let Some(parent_node) = self.nodes.get_mut(parent) {
                parent_node.children.push(spec.child_route_id.clone());
            }
        }
        let log_seq = self.log(
            "signal.spawn",
            principal,
            Some(&spec.child_route_id),
            json!({"depth":depth,"parents":parents,"origin":origin}),
        );
        let state = self.nodes[&spec.child_route_id].state.clone();
        Ok(json!({
            "child_route_id": spec.child_route_id,
            "depth": depth,
            "parents": parents,
            "origin": origin,
            "state": state,
            "log_seq": log_seq,
        }))
    }

    fn insert_grant(
        &mut self,
        holder: &str,
        target: &str,
        rights: BTreeSet<ControlRight>,
        granted_by: &str,
    ) {
        let at = now();
        let revision = self
            .grants
            .get(holder)
            .and_then(|by_target| by_target.get(target))
            .map(|grant| grant.revision + 1)
            .unwrap_or(1);
        let entry = self.grants.entry(holder.to_owned()).or_default();
        entry
            .entry(target.to_owned())
            .and_modify(|grant| {
                grant.rights = grant.rights.union(&rights).cloned().collect();
                grant.revision = revision;
                grant.granted_at = at;
            })
            .or_insert(ControlGrant {
                holder_route_id: holder.to_owned(),
                target_route_id: target.to_owned(),
                rights,
                granted_by_route_id: granted_by.to_owned(),
                granted_at: at,
                revision,
            });
    }

    /// Delegate a subset of the rights already held. Rights never widen.
    pub fn grant(
        &mut self,
        principal: &Principal,
        target: &str,
        holder: &str,
        rights: BTreeSet<ControlRight>,
    ) -> Result<Value> {
        let held = self.held(principal, target);
        if !held.contains(&ControlRight::Grant) {
            return Err(CoreError::forbidden(
                "granting control rights requires the grant right on the target",
            ));
        }
        if !self.nodes.contains_key(holder) && holder != principal.route_id {
            return Err(CoreError::invalid("grantee is not under Signal control"));
        }
        if rights.is_empty() {
            return Err(CoreError::invalid("grant requires at least one right"));
        }
        if !rights.is_subset(&held) {
            return Err(CoreError::forbidden(
                "cannot delegate control rights that are not held",
            ));
        }
        self.insert_grant(holder, target, rights.clone(), &principal.route_id);
        let log_seq = self.log(
            "signal.admin_grant",
            principal,
            Some(target),
            json!({"holder":holder,"rights":rights}),
        );
        Ok(json!({"target_route_id":target,"holder_route_id":holder,
            "rights":rights,"log_seq":log_seq}))
    }

    /// Revoke delegated rights. The delegator rights stay untouched.
    pub fn revoke(
        &mut self,
        principal: &Principal,
        target: &str,
        holder: &str,
    ) -> Result<Value> {
        let held = self.held(principal, target);
        if !held.contains(&ControlRight::Grant) {
            return Err(CoreError::forbidden(
                "revoking control rights requires the grant right on the target",
            ));
        }
        if holder == target {
            return Err(CoreError::invalid("self rights cannot be revoked"));
        }
        let removed = self
            .grants
            .get_mut(holder)
            .and_then(|by_target| by_target.remove(target));
        let log_seq = self.log(
            "signal.admin_revoke",
            principal,
            Some(target),
            json!({"holder":holder,"removed":removed.is_some()}),
        );
        Ok(json!({"target_route_id":target,"holder_route_id":holder,
            "revoked":removed.is_some(),"log_seq":log_seq}))
    }
    /// Signal-Pause. Now suspends immediately; AfterOutput lets an in-flight
    /// completion land first so a finished answer is never discarded.
    pub fn pause(
        &mut self,
        principal: &Principal,
        target: &str,
        mode: ApplyMode,
        wake_reason: Option<String>,
    ) -> Result<Value> {
        self.require(principal, target, ControlRight::Pause)
            .map_err(CoreError::forbidden)?;
        let at = now();
        let node = self
            .nodes
            .get_mut(target)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        if node.state.is_terminal() {
            return Err(CoreError::conflict("a terminal Agent cannot be paused"));
        }
        let previous = node.state.clone();
        let state = match (&node.state, &mode) {
            (ControlState::Paused { .. }, _) => node.state.clone(),
            (_, ApplyMode::AfterOutput) => ControlState::PausePending { since: at },
            (_, ApplyMode::Now) => ControlState::Paused { since: at, wake_reason },
        };
        let transitioned = state != previous;
        node.state = state.clone();
        // Only a settled pause takes authority away from an in-flight attempt;
        // a pending one defers to the output boundary by design.
        if transitioned && !matches!(mode, ApplyMode::AfterOutput) {
            node.control_epoch += 1;
        }
        let epoch = node.control_epoch;
        let pending = node.pending();
        let log_seq = self.log(
            "signal.pause",
            principal,
            Some(target),
            json!({"mode":mode,"state":state,"queued":pending}),
        );
        Ok(json!({"target_route_id":target,"state":state,
            "control_epoch":epoch,"queued":pending,"log_seq":log_seq}))
    }

    /// Signal-Stop. An empty wake set is terminal; otherwise only the listed
    /// registered sources may ever revive the route. Nothing is discarded:
    /// arrivals during the stop are queued with their timestamps.
    pub fn stop(
        &mut self,
        principal: &Principal,
        target: &str,
        mode: ApplyMode,
        wake_sources: BTreeSet<String>,
    ) -> Result<Value> {
        self.require(principal, target, ControlRight::Stop)
            .map_err(CoreError::forbidden)?;
        for source_id in &wake_sources {
            if !self.sources.contains_key(source_id) {
                return Err(CoreError::invalid(
                    "a wake source must be registered before it can be admitted",
                ));
            }
        }
        let terminal = wake_sources.is_empty();
        let at = now();
        let node = self
            .nodes
            .get_mut(target)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        if node.state.is_terminal() {
            return Err(CoreError::conflict("Agent is already terminal"));
        }
        let state = match mode {
            ApplyMode::Now => ControlState::Stopped { since: at, terminal },
            ApplyMode::AfterOutput => ControlState::StopPending { since: at },
        };
        node.state = state.clone();
        // after_output deliberately keeps the current completion: bumping the
        // epoch here would discard exactly what the caller asked to preserve.
        if !matches!(mode, ApplyMode::AfterOutput) {
            node.control_epoch += 1;
        }
        let revision = node.policy.revision + 1;
        node.policy = SourcePolicy {
            kinds: BTreeSet::new(),
            source_ids: wake_sources.clone(),
            updated_at: at,
            revision,
        };
        let epoch = node.control_epoch;
        let pending = node.pending();
        let log_seq = self.log(
            "signal.stop",
            principal,
            Some(target),
            json!({"mode":mode,"terminal":terminal,
                  "wake_sources":wake_sources,"queued":pending}),
        );
        Ok(json!({"target_route_id":target,"state":state,
            "terminal":terminal,"wake_sources":wake_sources,
            "control_epoch":epoch,"queued":pending,"log_seq":log_seq}))
    }

    /// Self Sleep/Wait is a Pause with a declared wake deadline, never a Stop
    /// of itself: a Stop would drop the information that is still arriving.
    pub fn sleep(
        &mut self,
        principal: &Principal,
        timeout_s: Option<u64>,
        reason: Option<String>,
    ) -> Result<Value> {
        let target = principal.route_id.clone();
        let source_id = format!("clock:{}:{timeout_s:?}", target);
        self.register_source(principal, &source_id, SourceKind::ClockTimeout, None)?;
        let mut ids = BTreeSet::new();
        ids.insert(source_id.clone());
        self.set_source_policy(principal, &target, BTreeSet::new(), ids)?;
        let mut result = self.pause(principal, &target, ApplyMode::Now, reason)?;
        let log_seq = self.log(
            "signal.sleep",
            principal,
            Some(&target),
            json!({"timeout_s":timeout_s,"clock_source":source_id}),
        );
        result["timeout_s"] = json!(timeout_s);
        result["clock_source"] = json!(source_id);
        result["log_seq"] = json!(log_seq);
        Ok(result)
    }

    /// Admit a registered source. While the Agent is suspended this either
    /// contributes to a permitted wake or queues with its timestamp.
    pub fn arrive(&mut self, source_id: &str, target: &str, payload: Value) -> Result<Value> {
        let registration = self.sources.get(source_id).cloned().ok_or_else(|| {
            CoreError::new(
                "unregistered_source",
                "information sources must be registered before admission",
                409,
            )
        })?;
        let at = now();
        let global = self.next_global_seq;
        self.next_global_seq += 1;
        let node = self
            .nodes
            .get_mut(target)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        let current = node.state.clone();
        let (admitted, wake) = match &current {
            ControlState::Running | ControlState::PausePending { .. } => (true, false),
            ControlState::StopPending { .. } => (true, false),
            ControlState::Paused { .. } | ControlState::Stopped { .. } => {
                let allowed = node.policy.admits(&registration);
                (allowed, allowed)
            }
        };
        let seq = node.allocate();
        let item = QueuedItem {
            seq,
            received_at: at,
            source_id: source_id.to_owned(),
            kind: registration.kind.clone(),
            payload,
            admitted_at: if admitted { Some(at) } else { None },
            wake_eligible: wake,
        };
        node.queue.push(item);
        if node.queue.len() > 4096 {
            let excess = node.queue.len() - 4096;
            node.queue.drain(0..excess);
        }
        let mut resumed = false;
        if wake {
            node.state = ControlState::Running;
            node.control_epoch += 1;
            resumed = true;
        }
        let pending = node.pending();
        let epoch = node.control_epoch;
        let resulting = node.state.clone();
        if resumed {
            let system = Principal {
                actor_id: "system".into(),
                route_id: source_id.to_owned(),
            };
            self.log(
                "signal.wake",
                &system,
                Some(target),
                json!({"source_id":source_id,"global_seq":global}),
            );
        }
        Ok(json!({"target_route_id":target,"source_id":source_id,
            "admitted":admitted,"queued":!admitted,"resumed":resumed,
            "state":resulting,"queue_depth":pending,
            "control_epoch":epoch,"received_at":at,"seq":seq}))
    }

    /// Drain admitted items in arrival order. A queued item is never dropped.
    pub fn drain_admitted(&mut self, route_id: &str, limit: usize) -> Result<Value> {
        let node = self
            .nodes
            .get_mut(route_id)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        let admitted: Vec<QueuedItem> = node
            .queue
            .iter()
            .filter(|item| item.admitted_at.is_some())
            .take(limit.max(1))
            .cloned()
            .collect();
        let drained: BTreeSet<u64> = admitted.iter().map(|item| item.seq).collect();
        node.queue.retain(|item| !drained.contains(&item.seq));
        let remaining = node.pending();
        Ok(json!({"items":admitted,"remaining_queue":remaining}))
    }

    /// Settle a pending pause or stop at the model output boundary.
    pub fn settle_after_output(&mut self, route_id: &str) -> Option<ControlState> {
        let at = now();
        let node = self.nodes.get_mut(route_id)?;
        let next = match &node.state {
            ControlState::PausePending { .. } => ControlState::Paused {
                since: at,
                wake_reason: Some("after_output".into()),
            },
            ControlState::StopPending { .. } => {
                let terminal = !node.policy.is_open();
                ControlState::Stopped { since: at, terminal }
            }
            _ => return None,
        };
        node.state = next.clone();
        node.control_epoch += 1;
        Some(next)
    }

    /// Report for one Agent, including held rights and delegations.
    pub fn status(&self, route_id: &str) -> Result<Value> {
        let node = self
            .nodes
            .get(route_id)
            .ok_or_else(|| CoreError::invalid("Agent route is not under Signal control"))?;
        let children: Vec<Value> = node
            .children
            .iter()
            .filter_map(|child| {
                let child_node = self.nodes.get(child)?;
                Some(json!({
                    "route_id": child,
                    "state": child_node.state,
                    "depth": child_node.depth,
                    "queued": child_node.pending(),
                }))
            })
            .collect();
        let me = Principal {
            actor_id: node.route_id.clone(),
            route_id: node.route_id.clone(),
        };
        Ok(json!({
            "route_id": node.route_id,
            "state": node.state,
            "depth": node.depth,
            "parents": node.parents,
            "origin": node.origin,
            "children": children,
            "limits": node.limits,
            "policy": node.policy,
            "queued": node.pending(),
            "control_epoch": node.control_epoch,
            "created_at": node.created_at,
            "held_rights": self.held(&me, &node.route_id),
            "delegations": self.delegations_for(&node.route_id),
        }))
    }

    fn delegations_for(&self, route_id: &str) -> Vec<Value> {
        let mut rows = Vec::new();
        for (holder, by_target) in &self.grants {
            if let Some(grant) = by_target.get(route_id) {
                rows.push(json!({"holder_route_id":holder,
                    "rights":grant.rights,"revision":grant.revision,
                    "granted_by":grant.granted_by_route_id,
                    "granted_at":grant.granted_at}));
            }
        }
        rows
    }

    /// The I-region broadcast published at each CB overwrite: the management
    /// log with timestamps plus the current child states.
    pub fn i_broadcast(&self, route_id: &str, log_limit: usize) -> Value {
        let node = self.nodes.get(route_id);
        let children: Vec<Value> = match node {
            Some(n) => n
                .children
                .iter()
                .filter_map(|child| {
                    let child_node = self.nodes.get(child)?;
                    let last = self
                        .log
                        .iter()
                        .rev()
                        .find(|entry| entry.target_route_id.as_deref() == Some(child.as_str()))
                        .map(|entry| entry.seq);
                    Some(json!({
                        "route_id": child,
                        "state": child_node.state,
                        "depth": child_node.depth,
                        "since": child_node.state.since(),
                        "queued": child_node.pending(),
                        "last_log_seq": last,
                    }))
                })
                .collect(),
            None => Vec::new(),
        };
        let take = log_limit.clamp(1, 64);
        let recent: Vec<LogEntry> = self.log.iter().rev().take(take).cloned().collect();
        let ordered: Vec<LogEntry> = recent.into_iter().rev().collect();
        json!({
            "signal_state": node.map(|n| n.state.clone()),
            "children": children,
            "management_log": ordered,
            "queue_depth": node.map(|n| n.pending()).unwrap_or(0),
            "control_epoch": node.map(|n| n.control_epoch).unwrap_or(0),
        })
    }
}

/// Transactional persistence for the Signal aggregate. It uses its own table
/// and namespace so it never rewrites the existing IPC aggregate row.
#[derive(Clone)]
pub struct SignalStore {
    store: Store,
    namespace: String,
}

impl SignalStore {
    pub fn new(store: Store, namespace: impl Into<String>) -> Result<Self> {
        let namespace = namespace.into();
        if namespace.trim().is_empty() {
            return Err(CoreError::invalid("Signal namespace must be nonempty"));
        }
        Ok(Self { store, namespace })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub async fn load(&self) -> Result<SignalState> {
        let mut tx = self.store.begin().await?;
        Self::ensure_schema(&mut tx).await?;
        let state = self.load_in_transaction(&mut tx).await?;
        tx.commit().await?;
        Ok(state)
    }

    pub async fn transact<T, F>(&self, operation: F) -> Result<T>
    where
        F: FnOnce(&mut SignalState) -> Result<T>,
    {
        let mut tx = self.store.begin().await?;
        let result = self.transact_in_transaction(&mut tx, operation).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Apply a Signal transition inside a caller-owned transaction. If this
    /// returns an error the caller must abort its transaction.
    pub async fn transact_in_transaction<T, F>(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        operation: F,
    ) -> Result<T>
    where
        F: FnOnce(&mut SignalState) -> Result<T>,
    {
        Self::ensure_schema(tx).await?;
        let mut state = self.load_in_transaction(tx).await?;
        let result = operation(&mut state)?;
        let value = serde_json::to_value(&state)
            .map_err(|_| CoreError::invalid("Signal state cannot be serialized"))?;
        let encoded = storage::json_text(&value)?;
        let digest = storage::sha256(encoded.as_bytes());
        sqlx::query(
            "INSERT INTO sic_signal_states(namespace,state,digest) VALUES(?,?,?) \
             ON CONFLICT(namespace) DO UPDATE SET state=excluded.state,digest=excluded.digest",
        )
        .bind(&self.namespace)
        .bind(encoded)
        .bind(digest)
        .execute(&mut **tx)
        .await?;
        Ok(result)
    }

    async fn ensure_schema(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sic_signal_states(\
             namespace TEXT PRIMARY KEY, state TEXT NOT NULL, digest TEXT NOT NULL)",
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn load_in_transaction(&self, tx: &mut Transaction<'_, Sqlite>) -> Result<SignalState> {
        let row = sqlx::query("SELECT state,digest FROM sic_signal_states WHERE namespace=?")
            .bind(&self.namespace)
            .fetch_optional(&mut **tx)
            .await?;
        let Some(row) = row else {
            return Ok(SignalState::new());
        };
        let encoded: String = row.try_get("state")?;
        let expected_digest: String = row.try_get("digest")?;
        if storage::sha256(encoded.as_bytes()) != expected_digest {
            return Err(CoreError::new(
                "integrity_error",
                "Signal state digest mismatch",
                500,
            ));
        }
        let value: Value = serde_json::from_str(&encoded).map_err(|_| {
            CoreError::new("integrity_error", "Signal state is invalid JSON", 500)
        })?;
        serde_json::from_value(value).map_err(|_| {
            CoreError::new("integrity_error", "Signal state schema is invalid", 500)
        })
    }
}
