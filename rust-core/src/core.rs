//! Transactional SIC content lifecycle. The initial profile has one Editor;
//! unsupported capabilities are rejected instead of being reported as done.
use crate::{
    board::{BoardAcl, BoardActor, BoardService, TaskStatus},
    decision, editing,
    error::{CoreError, Result},
    ipc::{Lifecycle, Principal, RouteStatus, SignalAction, SocketAcl},
    ipc_store::IpcStore,
    model_policy, prefix, protocol, provider,
    storage::{self, Store},
    tool_dispatch::{self, AuthenticatedWorkerScope, ToolDispatch},
    tool_store::ToolStore,
    toolset::{ExecutionLease, ToolAcl, ToolDescriptor},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{Row, Sqlite, SqliteConnection, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationContext {
    pub actor: String,
    #[serde(default)]
    pub chain_id: Option<String>,
    #[serde(default)]
    pub allow_write: bool,
    #[serde(default)]
    pub allow_model: bool,
    #[serde(default)]
    pub allow_revoke: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Level {
    wbs: Vec<String>,
    cbs: Vec<String>,
    pending: Vec<String>,
    pairs: BTreeMap<String, String>,
    instant: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct InternalContext {
    levels: BTreeMap<u32, Level>,
    shared_contexts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct View {
    context_id: String,
    level: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    job_id: String,
    source_id: String,
    wb_id: String,
    context_id: String,
    level: u32,
    base_cut: u64,
    source_snapshot: Value,
    assignments: Value,
    responses: Value,
    status: String,
    response_id: Option<String>,
    candidate: Option<Value>,
    candidate_revision: u64,
    candidate_revision_id: Option<String>,
    cb_id: Option<String>,
    #[serde(default)]
    decision_revision: u64,
    #[serde(default)]
    model_profile: Option<Value>,
    #[serde(default)]
    frozen_editor_config: Value,
    #[serde(default)]
    claim_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Agent {
    chain_id: String,
    owners: Vec<String>,
    creators: Vec<String>,
    origin_checkpoint: Option<String>,
    groups: BTreeMap<String, Vec<String>>,
    contexts: BTreeMap<String, InternalContext>,
    role_views: BTreeMap<String, View>,
    jobs: BTreeMap<String, Job>,
    protected: Vec<Value>,
    current_cut: Option<String>,
    cut_seq: u64,
    run_state: String,
    config: Value,
    #[serde(default = "decision::empty")]
    decision: Value,
    #[serde(default)]
    worker_claim: Option<String>,
    #[serde(default)]
    board_id: Option<String>,
    #[serde(default)]
    control_epoch: u64,
    #[serde(default)]
    input_epoch: u64,
    events: Vec<Value>,
}

impl Agent {
    fn new(actor: &str, creators: Vec<String>) -> Self {
        let mut main = InternalContext::default();
        main.levels.insert(1, Level::default());
        let mut contexts = BTreeMap::new();
        contexts.insert("main".into(), main);
        let view = View {
            context_id: "main".into(),
            level: 1,
        };
        let mut role_views = BTreeMap::new();
        role_views.insert("worker".into(), view.clone());
        role_views.insert("editor".into(), view);
        Self {
            chain_id: storage::uid("chain"),
            owners: vec![actor.into()],
            creators,
            origin_checkpoint: None,
            groups: BTreeMap::new(),
            contexts,
            role_views,
            jobs: BTreeMap::new(),
            protected: Vec::new(),
            current_cut: None,
            cut_seq: 0,
            run_state: "running".into(),
            config: json!({"prefix_layout":"shared_i","schedule_policy":"wait","editor_feedback":{"max_corrections":2,"feedback_chars":8192}}),
            decision: decision::empty(),
            worker_claim: None,
            board_id: None,
            control_epoch: 0,
            input_epoch: 0,
            events: Vec::new(),
        }
    }
    fn level(&self, ctx: &str, level: u32) -> Result<&Level> {
        self.contexts
            .get(ctx)
            .and_then(|c| c.levels.get(&level))
            .ok_or_else(|| CoreError::invalid("context or level not found"))
    }
    fn level_mut(&mut self, ctx: &str, level: u32) -> Result<&mut Level> {
        if level == 0 {
            return Err(CoreError::invalid("positive level required"));
        }
        let context = self
            .contexts
            .get_mut(ctx)
            .ok_or_else(|| CoreError::invalid("context not found"))?;
        Ok(context.levels.entry(level).or_default())
    }
}

#[derive(Clone)]
pub struct Core {
    pub store: Store,
    model_config: Option<Arc<Value>>,
    secret_resolver: Arc<dyn Fn(&str) -> std::result::Result<String, String> + Send + Sync>,
}

const OPERATIONS: &[(&str, bool, bool)] = &[
    ("chain.create", true, false),
    ("chain.list", false, false),
    ("chain.get", false, false),
    ("chain.checkpoint", true, false),
    ("socket.create", true, false),
    ("socket.acl.update", true, false),
    ("socket.read", false, false),
    ("socket.spawn", true, false),
    ("socket.message", true, false),
    ("socket.signal", true, false),
    ("socket.inbox", false, false),
    // Peer task controls are host-plane calls; listing is actor and route scoped.
    ("peer.tasks", false, false),
    ("peer.task.claim", true, false),
    ("peer.task.prepare", true, true),
    ("peer.task.reply", true, false),
    ("peer.task.advance", true, false),
    ("peer.task.unknown", true, false),
    ("board.create", true, false),
    ("board.bind", true, false),
    ("board.unbind", true, false),
    ("board.get", false, false),
    ("board.acl.update", true, false),
    ("board.entry.text", true, false),
    ("board.entry.data", true, false),
    ("board.task.create", true, false),
    ("board.task.claim", true, false),
    ("board.task.release", true, false),
    ("board.task.progress", true, false),
    ("board.task.transition", true, false),
    ("board.history", false, false),
    ("tools.configure", true, false),
    ("tools.sync", true, false),
    ("tools.update_acl", true, false),
    ("tools.register", true, false),
    ("tools.status", false, false),
    ("tools.tasks", false, false),
    ("tool.claim", true, false),
    ("tool.complete", true, false),
    ("source.ingest", true, false),
    ("source.read", false, false),
    ("wb.form", true, false),
    ("edit.prepare", true, false),
    ("edit.program", true, false),
    ("edit.submit", true, false),
    ("edit.run", true, true),
    ("edit.validate", false, false),
    ("edit.candidate.get", false, false),
    ("edit.candidate.update", true, false),
    ("worker.run", true, true),
    ("runtime.abandon", true, false),
    ("runtime.schedule", true, false),
    ("accounting.get", false, false),
    ("legacy.status", false, false),
    ("legacy.queue", false, false),
    ("legacy.activation.audit", false, false),
    ("legacy.activation.apply", true, false),
    ("legacy.activation.start", true, false),
    ("cut.publish", true, false),
    ("cut.get", false, false),
    ("memory.page", false, false),
    ("memory.replay", false, false),
    ("hierarchy.get", false, false),
    ("hierarchy.form", true, false),
    ("hierarchy.timeline", false, false),
    ("hierarchy.expand", false, false),
    ("hierarchy.select", true, false),
    ("hierarchy.context.create", true, false),
    ("hierarchy.context.ingest", true, false),
    ("decision.get", false, false),
    ("decision.update", true, false),
    ("config.get", false, false),
    ("config.update", true, false),
    ("security.revoke", true, false),
];

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CoreError::invalid(format!("{key} must be a nonempty string")))
}
fn field_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::invalid(format!("{key} must be a nonnegative integer")))
}
fn field_level(value: &Value) -> Result<u32> {
    let n = field_u64(value, "level")?;
    if n == 0 || n > 128 {
        return Err(CoreError::invalid("level must be 1..128"));
    }
    Ok(n as u32)
}
fn obj(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| CoreError::invalid("payload must be an object"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn as_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| CoreError::invalid("state serialization failed"))
}
fn as_agent(value: Value) -> Result<Agent> {
    serde_json::from_value(value).map_err(|_| {
        CoreError::new(
            "incompatible_state",
            "chain is not a supported Rust state",
            409,
        )
    })
}
fn err(message: String) -> CoreError {
    CoreError::invalid(message)
}

fn decode_source(format: &str, request: &Value) -> Result<Value> {
    let decoded = protocol::decode(format, request).map_err(err)?;
    // The protocol sidecar can round-trip inline images, but the current
    // Editor/Worker projection materializes only text. Do not accept media
    // here until it can survive publication and reach both model roles.
    if format == "openai-chat"
        && request["messages"].as_array().is_some_and(|messages| {
            messages.iter().any(|message| {
                message["content"].as_array().is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|part| part["type"].as_str() == Some("image_url"))
                })
            })
        })
    {
        return Err(CoreError::new(
            "media_not_implemented",
            "image input cannot yet be preserved through Editor and Worker",
            501,
        ));
    }
    let units:Vec<Value>=decoded.units.iter().map(|unit|json!({"unit_id":unit.unit_id,"text":unit.text,"anchor":{"native_path":unit.anchor},"kind":"text"})).collect();
    let mut protected = Vec::new();
    let mut working = false;
    let mut calls = BTreeSet::new();
    let mut results = BTreeSet::new();
    if format == "openai-chat" {
        for message in request["messages"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("messages required"))?
        {
            let role = message["role"].as_str().unwrap_or("");
            if role == "system" || role == "developer" {
                if working {
                    return Err(CoreError::invalid(
                        "protected instructions must precede working material",
                    ));
                }
                protected.push(message.clone());
            } else {
                working = true;
            }
            if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in tool_calls {
                    calls.insert(required_str(call, "id")?.to_owned());
                }
            }
            if role == "tool" {
                results.insert(required_str(message, "tool_call_id")?.to_owned());
            }
        }
    }
    let open: Vec<String> = calls.difference(&results).cloned().collect();
    Ok(
        json!({"units":units,"sidecar":decoded.sidecar(),"protected":protected,"open_calls":open,"format":format}),
    )
}

async fn load_agent(conn: &mut SqliteConnection, id: &str, actor: &str) -> Result<Agent> {
    as_agent(storage::get_chain(conn, id, actor).await?)
}
async fn save_agent(conn: &mut SqliteConnection, agent: &Agent) -> Result<()> {
    storage::save_chain(conn, &as_value(agent)?).await
}
async fn check_dependencies(conn: &mut SqliteConnection, agent: &Agent) -> Result<()> {
    for sources in agent.groups.values() {
        for id in sources {
            storage::get_object(conn, id, Some("source")).await?;
        }
    }
    Ok(())
}
async fn source_snapshot(
    conn: &mut SqliteConnection,
    agent: &Agent,
    root: &str,
    limited: Option<&[String]>,
) -> Result<Value> {
    let ids = agent
        .groups
        .get(root)
        .ok_or_else(|| CoreError::forbidden("source not on this chain"))?;
    let selected = limited.unwrap_or(ids);
    if selected.is_empty() || !ids.starts_with(selected) {
        return Err(CoreError::conflict("frozen WB sources no longer match"));
    }
    let mut units = Vec::new();
    let mut open = Vec::new();
    for (position, id) in selected.iter().enumerate() {
        let part = storage::get_object(conn, id, Some("source")).await?;
        for unit in part["units"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("source units missing"))?
        {
            let mut unit = unit.clone();
            let current = required_str(&unit, "unit_id")?.to_owned();
            if position > 0 {
                unit["unit_id"] = json!(format!("{id}:{current}"));
            }
            let anchor = unit.get("anchor").cloned().unwrap_or(Value::Null);
            unit["anchor"] = json!({"source_id":id,"native":anchor});
            units.push(unit);
        }
        // Each appended native fragment records the open calls after that
        // fragment; earlier open calls may already have been closed.
        open = part["open_calls"].as_array().cloned().unwrap_or_default();
    }
    Ok(json!({"units":units,"source_ids":selected,"open_calls":open}))
}
async fn register_wb(
    conn: &mut SqliteConnection,
    agent: &mut Agent,
    context_id: &str,
    level: u32,
    source_id: &str,
    members: Vec<String>,
    reason: &str,
) -> Result<String> {
    let existing = agent.level(context_id, level)?.wbs.clone();
    for wid in &existing {
        let wb = storage::get_object(conn, wid, Some("wb")).await?;
        if wb["source_id"].as_str() == Some(source_id) {
            return Ok(wid.clone());
        }
    }
    let source = source_snapshot(conn, agent, source_id, None).await?;
    let wid = storage::uid("wb");
    let sequence = existing.len() + 1;
    let units: Vec<Value> = source["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["unit_id"].clone())
        .collect();
    let wb = json!({"wb_id":wid,"chain_id":agent.chain_id,"context_id":context_id,"level":level,"sequence":sequence,
                  "source_id":source_id,"source_ids":source["source_ids"],"unit_ids":units,"members":members,"reason":reason,"formed_at":now()});
    storage::put_object(conn, "wb", &wid, &wb).await?;
    agent.level_mut(context_id, level)?.wbs.push(wid.clone());
    Ok(wid)
}

fn deterministic_ops(source: &Value, rules: &Value) -> Result<Vec<Value>> {
    let rules = rules
        .as_array()
        .ok_or_else(|| CoreError::invalid("rules must be an array"))?;
    let mut selected = BTreeSet::new();
    for rule in rules {
        let rule = rule
            .as_str()
            .ok_or_else(|| CoreError::invalid("rule must be a string"))?;
        if !["deduplicate_exact", "drop_empty", "normalize_newlines"].contains(&rule) {
            return Err(CoreError::invalid("unknown deterministic rule"));
        }
        selected.insert(rule);
    }
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for unit in source["units"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("source units missing"))?
    {
        let id = required_str(unit, "unit_id")?;
        let text = unit["text"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("unit text missing"))?;
        if selected.contains("drop_empty") && text.trim().is_empty() {
            operations.push(json!({"op":"DELETE","target":id,"reason":"empty text"}));
        } else if selected.contains("deduplicate_exact") && seen.contains(text) {
            operations
                .push(json!({"op":"DELETE","target":id,"reason":"exact duplicate within WB"}));
        } else if selected.contains("normalize_newlines") && text.contains('\r') {
            operations.push(json!({"op":"REPLACE","target":id,"content":text.replace("\r\n","\n").replace('\r',"\n"),"reason":"deterministic newline normalization"}));
        }
        seen.insert(text.to_owned());
    }
    Ok(operations)
}

impl Core {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            model_config: None,
            secret_resolver: Arc::new(|name| {
                std::env::var(name).map_err(|_| "missing host credential".into())
            }),
        }
    }
    /// Only the host can install provider profiles. Request bodies cannot
    /// change endpoint, identity, credentials, or token limits.
    pub fn with_model_config(store: Store, config: Value) -> Result<Self> {
        Self::with_model_config_and_resolver(
            store,
            config,
            Arc::new(|name| std::env::var(name).map_err(|_| "missing host credential".into())),
        )
    }
    pub fn with_model_config_and_resolver(
        store: Store,
        config: Value,
        resolver: Arc<dyn Fn(&str) -> std::result::Result<String, String> + Send + Sync>,
    ) -> Result<Self> {
        model_policy::validate_config(&config).map_err(err)?;
        let worker = model_policy::resolve_worker_profile(&config).map_err(err)?;
        let editor = model_policy::resolve_editor_profile(&config, 1, "editor-1").map_err(err)?;
        provider::validate_openai_profile(&worker).map_err(err)?;
        provider::validate_openai_profile(&editor).map_err(err)?;
        Ok(Self {
            store,
            model_config: Some(Arc::new(config)),
            secret_resolver: resolver,
        })
    }
    pub fn capabilities() -> Value {
        json!({"api_version":"2","implementation":"sic-rust-core-development","operations":OPERATIONS.iter().map(|(op,write,model)|json!({"operation":op,"write":write,"model":model})).collect::<Vec<_>>(),
               "feature_scope":"single Editor; implemented operations only; not complete contract acceptance","physical_cache_hit":"unknown"})
    }
    /// Return the canonical fixed SIC tool names for adapters that must reserve
    /// every common operation without maintaining a duplicate name list.
    pub fn common_tool_names() -> Vec<String> {
        prefix::common_tool_registry()
            .into_iter()
            .filter_map(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect()
    }

    /// Revalidate a prepared peer immediately before a host reserves its
    /// one-shot DSH process launch. The caller holds the same transaction used
    /// for the launch intent; a prior prepare receipt is not a current grant.
    pub(crate) async fn verify_peer_process_launch(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor: &str,
        chain_id: &str,
    ) -> Result<()> {
        let agent = load_agent(tx, chain_id, actor).await?;
        check_dependencies(tx, &agent).await?;
        self.check_board_access(tx, &agent).await?;
        if agent.run_state != "running"
            || agent.worker_claim.is_some()
            || agent.current_cut.is_none()
            || agent.contexts.values().any(|context| {
                context
                    .levels
                    .values()
                    .any(|level| !level.pending.is_empty())
            })
            || agent.jobs.values().any(|job| {
                job.status == "inflight"
                    || (job.base_cut == agent.cut_seq
                        && matches!(job.status.as_str(), "open" | "ready" | "failed"))
            })
        {
            return Err(CoreError::conflict(
                "prepared peer must be running and idle before host process launch",
            ));
        }
        for cb_id in self.selected_ids(tx, &agent, "worker").await? {
            storage::get_object(tx, &cb_id, Some("cb")).await?;
        }
        Ok(())
    }

    pub async fn operate(
        &self,
        operation: &str,
        context: &OperationContext,
        payload: &Value,
    ) -> Result<Value> {
        obj(payload)?;
        if context.actor.is_empty() {
            return Err(CoreError::forbidden("actor required"));
        }
        let (_, write, model) = OPERATIONS
            .iter()
            .find(|(name, _, _)| *name == operation)
            .ok_or_else(|| {
                CoreError::new(
                    "unsupported_operation",
                    "operation is not implemented in this Rust build",
                    501,
                )
            })?;
        if *write && !context.allow_write {
            return Err(CoreError::new(
                "write_denied",
                "write authorization required",
                403,
            ));
        }
        if *model && !context.allow_model {
            return Err(CoreError::new(
                "model_denied",
                "model authorization required",
                403,
            ));
        }
        if operation == "security.revoke" && !context.allow_revoke {
            return Err(CoreError::new(
                "revoke_denied",
                "separate revocation authorization required",
                403,
            ));
        }
        if operation == "edit.run" {
            return self.run_editor(context, payload).await;
        }
        if operation == "worker.run" {
            return self.run_worker(context, payload).await;
        }
        if operation == "runtime.schedule" {
            let policy: crate::scheduler::SchedulerPolicy =
                serde_json::from_value(payload.clone())?;
            let report = Box::pin(crate::scheduler::schedule(self, context, &policy)).await;
            return Ok(
                json!({"ok":true,"operation":operation,"context":{"chain_id":context.chain_id,"actor":context.actor},
                "evidence":{"provider_called":report.model_calls_observed>0,"provider_calls_unknown":report.model_calls_reserved_unknown,
                            "physical_cache_hit":"unknown"},"result":report}),
            );
        }
        if operation == "peer.task.prepare" {
            return self.prepare_peer_task(context, payload).await;
        }
        if matches!(
            operation,
            "legacy.activation.audit" | "legacy.activation.apply" | "legacy.activation.start"
        ) {
            // The migration module rechecks the archived and live owner in
            // the same transaction as the ACL update or receipt replay.
            let chain_id = context
                .chain_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CoreError::invalid("chain_id required"))?;
            let result = if operation == "legacy.activation.audit" {
                as_value(
                    &crate::legacy_activation::audit_quarantined_activation(
                        &self.store,
                        chain_id,
                        &context.actor,
                    )
                    .await?,
                )?
            } else if operation == "legacy.activation.apply" {
                as_value(
                    &crate::legacy_activation::activate_verified_owner_board(
                        &self.store,
                        chain_id,
                        &context.actor,
                        field_u64(payload, "expected_board_revision")?,
                        required_str(payload, "idempotency_key")?,
                    )
                    .await?,
                )?
            } else {
                as_value(
                    &crate::legacy_activation::start_activated_agent(
                        &self.store,
                        chain_id,
                        &context.actor,
                        required_str(payload, "idempotency_key")?,
                    )
                    .await?,
                )?
            };
            return Ok(json!({"ok":true,"operation":operation,
                "context":{"chain_id":chain_id,"actor":context.actor},
                "evidence":{"provider_called":false,"physical_cache_hit":"unknown",
                            "replayed":result.get("replayed").and_then(Value::as_bool).unwrap_or(false)},
                "result":result}));
        }
        let mut tx = self.store.begin().await?;
        let mut replayed = false;
        let (chain_id, result) = if operation == "chain.create" {
            let creators = payload
                .get("creators")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut ids = Vec::new();
            for creator in creators {
                let id = creator
                    .as_str()
                    .ok_or_else(|| CoreError::invalid("creator must be a chain id"))?;
                load_agent(&mut tx, id, &context.actor).await?;
                ids.push(id.to_owned());
            }
            let agent = Agent::new(&context.actor, ids.clone());
            save_agent(&mut tx, &agent).await?;
            self.ensure_ipc_route(&mut tx, &agent, &context.actor)
                .await?;
            (
                Some(agent.chain_id.clone()),
                json!({"chain_id":agent.chain_id,"creators":ids}),
            )
        } else if operation == "chain.list" && context.chain_id.as_deref().is_none_or(str::is_empty)
        {
            let rows = sqlx::query("SELECT state FROM chains ORDER BY id")
                .fetch_all(&mut *tx)
                .await?;
            let mut routes = Vec::new();
            for row in rows {
                let value: String = row.try_get("state")?;
                let parsed: Value = serde_json::from_str(&value)?;
                if parsed["owners"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|x| x.as_str() == Some(&context.actor)))
                {
                    routes.push(json!({"chain_id":parsed["chain_id"],"cut_seq":parsed["cut_seq"],"run_state":parsed["run_state"]}));
                }
            }
            (None, json!({"routes":routes}))
        } else {
            let id = context
                .chain_id
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| CoreError::invalid("chain_id required"))?;
            let mut agent = load_agent(&mut tx, id, &context.actor).await?;
            if ![
                "security.revoke",
                "source.read",
                "tool.complete",
                "tools.tasks",
                "tools.status",
                "runtime.abandon",
                "peer.tasks",
                "peer.task.advance",
                "peer.task.unknown",
            ]
            .contains(&operation)
            {
                check_dependencies(&mut tx, &agent).await?;
            }
            if agent.board_id.is_some()
                && !operation.starts_with("board.")
                && ![
                    "security.revoke",
                    "source.read",
                    "tool.complete",
                    "tools.tasks",
                    "tools.status",
                    "runtime.abandon",
                    "peer.tasks",
                    "peer.task.advance",
                    "peer.task.unknown",
                ]
                .contains(&operation)
            {
                self.check_board_access(&mut tx, &agent).await?;
            }
            let result = if operation == "chain.checkpoint" || operation.starts_with("socket.") {
                self.dispatch_ipc(
                    &mut tx,
                    &mut agent,
                    context,
                    operation,
                    payload,
                    &mut replayed,
                )
                .await?
            } else if operation.starts_with("tools.")
                || operation == "tool.claim"
                || operation == "tool.complete"
            {
                self.dispatch_host_tool(
                    &mut tx,
                    &mut agent,
                    context,
                    operation,
                    payload,
                    &mut replayed,
                )
                .await?
            } else if operation == "peer.tasks" || operation.starts_with("peer.task.") {
                self.dispatch_peer_task(&mut tx, &agent, context, operation, payload, &mut replayed)
                    .await?
            } else if operation.starts_with("board.") {
                self.dispatch_board(&mut tx, &mut agent, operation, payload)
                    .await?
            } else if operation == "cut.publish" {
                self.publish_with_ipc(&mut tx, &mut agent, payload, &mut replayed)
                    .await?
            } else {
                self.dispatch(&mut tx, &mut agent, operation, payload, &mut replayed)
                    .await?
            };
            if *write && !replayed {
                save_agent(&mut tx, &agent).await?;
            }
            (Some(id.to_owned()), result)
        };
        tx.commit().await?;
        Ok(
            json!({"ok":true,"operation":operation,"context":{"chain_id":chain_id,"actor":context.actor},"result":result,
                  "evidence":{"physical_cache_hit":"unknown","provider_called":false,"replayed":replayed}}),
        )
    }

    async fn dispatch(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        operation: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        match operation {
            "chain.get" => as_value(agent),
            "accounting.get" => self.accounting(conn, agent).await,
            "legacy.status" | "legacy.queue" => self.legacy_status(conn, agent, operation, p).await,
            "runtime.abandon" => self.abandon_inflight(conn, agent, p).await,
            "chain.list" => Ok(
                json!({"routes":[{"chain_id":agent.chain_id,"cut_seq":agent.cut_seq,"run_state":agent.run_state}]}),
            ),
            "source.ingest" => self.ingest(conn, agent, "main", p, replayed).await,
            "hierarchy.context.ingest" => {
                let ctx = required_str(p, "context_id")?;
                if ctx == "main" {
                    return Err(CoreError::invalid("use source.ingest for main context"));
                }
                self.ingest(conn, agent, ctx, p, replayed).await
            }
            "source.read" => {
                let id = required_str(p, "source_id")?;
                if !agent.groups.values().any(|g| g.iter().any(|x| x == id)) {
                    return Err(CoreError::forbidden("source is not on this chain"));
                }
                Ok(
                    json!({"source_id":id,"source":storage::get_object(conn,id,Some("source")).await?}),
                )
            }
            "wb.form" => self.form_first_order_wb(conn, agent, p).await,
            "edit.prepare" | "edit.program" => {
                self.prepare(conn, agent, p, operation == "edit.program")
                    .await
            }
            "edit.submit" => self.submit(conn, agent, p).await,
            "edit.validate" => {
                let job = job(agent, p)?;
                let material = combined_material(conn, agent, job).await?;
                Ok(json!({"job_id":job.job_id,"base_cut":job.base_cut,"material":material}))
            }
            "edit.candidate.get" => {
                let job = job(agent, p)?;
                Ok(
                    json!({"job_id":job.job_id,"revision":job.candidate_revision,"candidate":job.candidate,"original_response_id":job.response_id}),
                )
            }
            "edit.candidate.update" => self.update_candidate(conn, agent, p).await,
            "cut.get" => self.get_cut(conn, agent).await,
            "hierarchy.get" => Ok(
                json!({"chain_id":agent.chain_id,"contexts":agent.contexts,"role_views":agent.role_views}),
            ),
            "hierarchy.context.create" => self.create_context(agent, p),
            "hierarchy.form" => self.form_higher(conn, agent, p).await,
            "hierarchy.timeline" => self.timeline(conn, agent, p).await,
            "hierarchy.expand" => self.expand(conn, agent, p).await,
            "hierarchy.select" => self.select(conn, agent, p).await,
            "memory.page" => self.page(conn, agent, p).await,
            "memory.replay" => self.replay(conn, agent, p).await,
            "decision.get" => Ok(json!({"decision":agent.decision})),
            "decision.update" => self.update_decision(conn, agent, p).await,
            "config.get" => Ok(json!({"configuration":agent.config})),
            "config.update" => self.update_config(agent, p),
            "security.revoke" => self.revoke(conn, agent, p).await,
            _ => Err(CoreError::new(
                "unsupported_operation",
                "operation is not implemented in this Rust build",
                501,
            )),
        }
    }

    async fn ingest(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        ctx: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        agent.level(ctx, 1)?;
        let format = p
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("openai-chat");
        if format != "openai-chat" {
            return Err(CoreError::new(
                "unsupported",
                "Core ingress currently supports Chat only; other protocol adapters are not yet wired",
                501,
            ));
        }
        let request = p
            .get("request")
            .ok_or_else(|| CoreError::invalid("request required"))?;
        let receipt = if let Some(value) = p.get("idempotency_key") {
            let key = value
                .as_str()
                .filter(|x| !x.is_empty())
                .ok_or_else(|| CoreError::invalid("idempotency_key must be nonempty"))?;
            let mut payload = p.clone();
            payload.as_object_mut().unwrap().remove("idempotency_key");
            let fingerprint = storage::digest(&json!({"context_id":ctx,"payload":payload}))?;
            let receipt_key = format!("{}:source:{key}", agent.chain_id);
            if let Some(row) = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?")
                .bind(&receipt_key)
                .fetch_optional(&mut *conn)
                .await?
            {
                let prior: String = row.try_get("fingerprint")?;
                if prior != fingerprint {
                    return Err(CoreError::conflict(
                        "idempotency key reused with different source",
                    ));
                }
                let value: String = row.try_get("result")?;
                *replayed = true;
                return Ok(serde_json::from_str(&value)?);
            }
            Some((receipt_key, fingerprint))
        } else {
            None
        };
        let pending = agent.level(ctx, 1)?.pending.last().cloned();
        let open_root = if let Some(id) = pending {
            let source = source_snapshot(conn, agent, &id, None).await?;
            if source["open_calls"]
                .as_array()
                .is_some_and(|x| !x.is_empty())
            {
                Some(id)
            } else {
                None
            }
        } else {
            None
        };
        let source = if let Some(root) = &open_root {
            if ctx != "main" {
                return Err(CoreError::invalid(
                    "internal contexts cannot contain open native calls",
                ));
            }
            let mut merged = Vec::new();
            for source_id in agent
                .groups
                .get(root)
                .ok_or_else(|| CoreError::invalid("open source group missing"))?
            {
                let old = storage::get_object(conn, source_id, Some("source")).await?;
                merged.extend(
                    old["sidecar"]["messages"]
                        .as_array()
                        .ok_or_else(|| CoreError::invalid("native messages missing"))?
                        .iter()
                        .cloned(),
                );
            }
            let old_count = merged.len();
            let incoming = request["messages"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("messages required"))?;
            if incoming
                .iter()
                .any(|m| matches!(m["role"].as_str(), Some("system" | "developer")))
            {
                return Err(CoreError::invalid(
                    "protected instructions cannot occur in a tool continuation",
                ));
            }
            merged.extend(incoming.iter().cloned());
            let mut combined = request.clone();
            combined["messages"] = json!(merged);
            let decoded = protocol::decode("openai-chat", &combined).map_err(err)?;
            let mut units = Vec::new();
            for unit in decoded.units {
                let mut anchor = unit
                    .anchor
                    .as_array()
                    .cloned()
                    .ok_or_else(|| CoreError::invalid("invalid native anchor"))?;
                let index = anchor
                    .get(1)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| CoreError::invalid("invalid message anchor"))?
                    as usize;
                if index >= old_count {
                    anchor[1] = json!(index - old_count);
                    units.push(json!({"unit_id":format!("u{}",units.len()+1),"text":unit.text,"anchor":{"native_path":anchor},"kind":"text"}));
                }
            }
            let all = decode_source("openai-chat", &combined)?;
            json!({"units":units,"sidecar":request,"protected":[],"open_calls":all["open_calls"],"format":"openai-chat"})
        } else {
            decode_source(format, request)?
        };
        if ctx != "main"
            && (!source["protected"].as_array().unwrap().is_empty()
                || !source["open_calls"].as_array().unwrap().is_empty())
        {
            return Err(CoreError::invalid(
                "internal context must contain closed unprotected material",
            ));
        }
        let protected = source["protected"].as_array().unwrap();
        if !protected.is_empty() {
            if !agent.protected.is_empty() && &agent.protected != protected {
                return Err(CoreError::new(
                    "protected_prefix_conflict",
                    "protected instructions changed",
                    409,
                ));
            }
            if agent.current_cut.is_some() && &agent.protected != protected {
                return Err(CoreError::new(
                    "protected_prefix_conflict",
                    "cannot add protected instructions after publication",
                    409,
                ));
            }
            agent.protected = protected.clone();
        }
        let part_id = storage::uid("source");
        storage::put_object(conn, "source", &part_id, &source).await?;
        let root = if let Some(root) = open_root {
            agent
                .groups
                .get_mut(&root)
                .ok_or_else(|| CoreError::invalid("open group missing"))?
                .push(part_id.clone());
            root
        } else {
            agent.groups.insert(part_id.clone(), vec![part_id.clone()]);
            agent.level_mut(ctx, 1)?.pending.push(part_id.clone());
            part_id.clone()
        };
        agent.input_epoch = agent
            .input_epoch
            .checked_add(1)
            .ok_or_else(|| CoreError::conflict("input revision exhausted"))?;
        agent.events.push(json!({"type":"source_ingested","source_id":root,"part_source_id":part_id,"time":now()}));
        let units = source_snapshot(conn, agent, &root, None).await?["units"].clone();
        let result = json!({"chain_id":agent.chain_id,"context_id":ctx,"source_id":root,"part_source_id":part_id,"units":units,"open_calls":source["open_calls"]});
        if let Some((key, fingerprint)) = receipt {
            sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)")
                .bind(key)
                .bind(fingerprint)
                .bind(storage::json_text(&result)?)
                .execute(conn)
                .await?;
        }
        Ok(result)
    }

    async fn form_first_order_wb(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let ctx = p
            .get("context_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let ids = p
            .get("source_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| CoreError::invalid("source_ids required"))?;
        let selected: Vec<String> = ids
            .iter()
            .map(|x| {
                x.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| CoreError::invalid("invalid source id"))
            })
            .collect::<Result<_>>()?;
        if selected.is_empty() || !agent.level(ctx, 1)?.pending.starts_with(&selected) {
            return Err(CoreError::conflict(
                "WB must contain earliest consecutive pending sources",
            ));
        }
        for wid in &agent.level(ctx, 1)?.wbs {
            let wb = storage::get_object(conn, wid, Some("wb")).await?;
            if selected
                .iter()
                .any(|id| wb["source_id"].as_str() == Some(id))
            {
                return Err(CoreError::conflict(
                    "a selected source is already frozen in a WB",
                ));
            }
        }
        let root = selected[0].clone();
        let mut combined = Vec::new();
        for id in &selected {
            combined.extend(
                agent
                    .groups
                    .get(id)
                    .ok_or_else(|| CoreError::invalid("source group missing"))?
                    .clone(),
            );
        }
        for id in selected.iter().skip(1) {
            agent.groups.remove(id);
        }
        agent.groups.insert(root.clone(), combined);
        let level = agent.level_mut(ctx, 1)?;
        level.pending.drain(0..selected.len());
        level.pending.insert(0, root.clone());
        let reason = p
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("explicit-task-boundary");
        let wid = register_wb(conn, agent, ctx, 1, &root, Vec::new(), reason).await?;
        Ok(json!({"chain_id":agent.chain_id,"source_id":root,"wb_id":wid}))
    }

    async fn prepare(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
        program: bool,
    ) -> Result<Value> {
        if agent.run_state != "running" {
            return Err(CoreError::conflict("Agent is paused or stopped"));
        }
        if agent.jobs.values().any(|job| job.status == "inflight") {
            return Err(CoreError::conflict("the single Editor is already running"));
        }
        if agent.worker_claim.is_some() {
            return Err(CoreError::conflict(
                "Worker is still running on the current cut",
            ));
        }
        let source_id = required_str(p, "source_id")?;
        let (context_id, level) = agent
            .contexts
            .iter()
            .find_map(|(name, ctx)| {
                ctx.levels.iter().find_map(|(level, row)| {
                    row.pending
                        .contains(&source_id.to_owned())
                        .then(|| (name.clone(), *level))
                })
            })
            .ok_or_else(|| CoreError::invalid("source is not a pending WB"))?;
        if agent
            .level(&context_id, level)?
            .pending
            .first()
            .map(String::as_str)
            != Some(source_id)
        {
            return Err(CoreError::conflict("edit earliest uncommitted WB first"));
        }
        let existing = agent.level(&context_id, level)?.wbs.clone();
        let mut members = Vec::new();
        for wid in existing {
            let wb = storage::get_object(conn, &wid, Some("wb")).await?;
            if wb["source_id"].as_str() == Some(source_id) {
                members = serde_json::from_value(wb["members"].clone())?;
                break;
            }
        }
        let wb_id = register_wb(
            conn,
            agent,
            &context_id,
            level,
            source_id,
            members,
            "edit-boundary",
        )
        .await?;
        let source = source_snapshot(conn, agent, source_id, None).await?;
        let all: Vec<Value> = source["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["unit_id"].clone())
            .collect();
        let assignments = p
            .get("assignments")
            .cloned()
            .unwrap_or_else(|| json!([{"editor":"editor-1","unit_ids":all}]));
        let array = assignments
            .as_array()
            .ok_or_else(|| CoreError::invalid("assignments must be an array"))?;
        if array.len() != 1 {
            return Err(CoreError::invalid(
                "lean profile supports exactly one Editor",
            ));
        }
        let editor = required_str(&array[0], "editor")?.to_owned();
        if !array[0].get("unit_ids").is_some_and(Value::is_array) {
            return Err(CoreError::invalid("assigned unit_ids required"));
        }
        let id = storage::uid("job");
        let mut job = Job {
            job_id: id.clone(),
            source_id: source_id.into(),
            wb_id,
            context_id: context_id.clone(),
            level,
            base_cut: agent.cut_seq,
            source_snapshot: source,
            assignments,
            responses: json!({}),
            status: "open".into(),
            response_id: None,
            candidate: None,
            candidate_revision: 0,
            candidate_revision_id: None,
            cb_id: None,
            decision_revision: agent.decision["revision"]
                .as_u64()
                .ok_or_else(|| CoreError::invalid("decision revision missing"))?,
            model_profile: self
                .model_config
                .as_ref()
                .map(|config| model_policy::resolve_editor_profile(config, level, &editor))
                .transpose()
                .map_err(err)?,
            frozen_editor_config: agent.config.clone(),
            claim_id: None,
        };
        if program {
            let rules = p
                .get("rules")
                .cloned()
                .unwrap_or_else(|| json!(["deduplicate_exact"]));
            let ops = deterministic_ops(&job.source_snapshot, &rules)?;
            let response =
                json!({"ops":ops,"pvi":p.get("pvi").cloned().unwrap_or_else(||json!({}))});
            let mut responses = Map::new();
            responses.insert(editor, response.clone());
            editing::materialize(
                &job.source_snapshot,
                &job.assignments,
                &Value::Object(responses.clone()),
                context_id == "main" && level == 1,
            )
            .map_err(err)?;
            let rid = storage::uid("editor_response");
            storage::put_object(conn, "editor_response", &rid, &response).await?;
            job.responses = Value::Object(responses);
            job.response_id = Some(rid);
            job.candidate = Some(response);
            job.status = "ready".into();
        }
        let result = json!({"job_id":id,"job":job,"editable_units":job.source_snapshot["units"]});
        agent.jobs.insert(id, job);
        Ok(result)
    }

    async fn submit(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let job_id = required_str(p, "job_id")?;
        let editor = required_str(p, "editor")?;
        let job = agent
            .jobs
            .get_mut(job_id)
            .ok_or_else(|| CoreError::invalid("job not found"))?;
        if job.status != "open" {
            return Err(CoreError::conflict("Editor response already consumed"));
        }
        if job.assignments[0]["editor"].as_str() != Some(editor) {
            return Err(CoreError::forbidden("Editor not assigned"));
        }
        let response = json!({"ops":p.get("ops").cloned().unwrap_or_else(||json!([])),"pvi":p.get("pvi").cloned().unwrap_or_else(||json!({}))});
        let mut responses = Map::new();
        responses.insert(editor.to_owned(), response.clone());
        editing::materialize(
            &job.source_snapshot,
            &job.assignments,
            &Value::Object(responses.clone()),
            job.context_id == "main" && job.level == 1,
        )
        .map_err(err)?;
        let rid = storage::uid("editor_response");
        storage::put_object(conn, "editor_response", &rid, &response).await?;
        job.responses = Value::Object(responses);
        job.response_id = Some(rid.clone());
        job.candidate = Some(response);
        job.status = "ready".into();
        Ok(json!({"job_id":job_id,"status":"ready","response_id":rid}))
    }

    async fn update_candidate(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let id = required_str(p, "job_id")?;
        let expected = field_u64(p, "expected_revision")?;
        let job = agent
            .jobs
            .get_mut(id)
            .ok_or_else(|| CoreError::invalid("job not found"))?;
        if job.status != "ready" || job.candidate_revision != expected {
            return Err(CoreError::conflict(
                "candidate is not ready or revision changed",
            ));
        }
        let candidate = json!({"ops":p.get("ops").cloned().unwrap_or_else(||json!([])),"pvi":p.get("pvi").cloned().unwrap_or_else(||json!({}))});
        let editor = required_str(&job.assignments[0], "editor")?;
        let mut responses = Map::new();
        responses.insert(editor.to_owned(), candidate.clone());
        editing::materialize(
            &job.source_snapshot,
            &job.assignments,
            &Value::Object(responses),
            job.context_id == "main" && job.level == 1,
        )
        .map_err(err)?;
        let revision = job.candidate_revision + 1;
        let rid = storage::uid("candidate_revision");
        storage::put_object(conn,"candidate_revision",&rid,&json!({"job_id":id,"revision":revision,"candidate":candidate,"previous_revision_id":job.candidate_revision_id,"original_response_id":job.response_id})).await?;
        job.candidate = Some(candidate);
        job.candidate_revision = revision;
        job.candidate_revision_id = Some(rid.clone());
        Ok(json!({"job_id":id,"revision":revision,"revision_id":rid}))
    }
}

fn job<'a>(agent: &'a Agent, p: &Value) -> Result<&'a Job> {
    agent
        .jobs
        .get(required_str(p, "job_id")?)
        .ok_or_else(|| CoreError::invalid("job not found"))
}
fn job_material(job: &Job) -> Result<Value> {
    if job.status != "ready" {
        return Err(CoreError::conflict("Editor candidate not ready"));
    }
    let candidate = job
        .candidate
        .as_ref()
        .ok_or_else(|| CoreError::invalid("candidate missing"))?;
    let editor = required_str(&job.assignments[0], "editor")?;
    let mut responses = Map::new();
    responses.insert(editor.into(), candidate.clone());
    editing::materialize(
        &job.source_snapshot,
        &job.assignments,
        &Value::Object(responses),
        job.context_id == "main" && job.level == 1,
    )
    .map_err(err)
}

async fn combined_material(conn: &mut SqliteConnection, agent: &Agent, job: &Job) -> Result<Value> {
    if job.status != "ready" {
        return Err(CoreError::conflict("Editor candidate not ready"));
    }
    let current = source_snapshot(conn, agent, &job.source_id, None).await?;
    let frozen_ids: Vec<String> =
        serde_json::from_value(job.source_snapshot["source_ids"].clone())?;
    let current_ids: Vec<String> = serde_json::from_value(current["source_ids"].clone())?;
    if !current_ids.starts_with(&frozen_ids) {
        return Err(CoreError::conflict("frozen Editor sources changed"));
    }
    let frozen_units = job.source_snapshot["units"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("frozen units missing"))?;
    let current_units = current["units"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("current units missing"))?;
    if !current_units.starts_with(frozen_units) {
        return Err(CoreError::conflict("frozen Editor units changed"));
    }
    let candidate = job
        .candidate
        .as_ref()
        .ok_or_else(|| CoreError::invalid("candidate missing"))?;
    let editor = required_str(&job.assignments[0], "editor")?;
    let mut responses = Map::new();
    responses.insert(editor.into(), candidate.clone());
    let mut result = editing::materialize(
        &current,
        &job.assignments,
        &Value::Object(responses),
        job.context_id == "main" && job.level == 1,
    )
    .map_err(err)?;
    let late: Vec<String> = current_units[frozen_units.len()..]
        .iter()
        .map(|u| required_str(u, "unit_id").map(str::to_owned))
        .collect::<Result<_>>()?;
    if !late.is_empty() {
        let ops = result["ops"]
            .as_array_mut()
            .ok_or_else(|| CoreError::invalid("materialized operations missing"))?;
        for op in ops.iter_mut() {
            if op["origin"] == "implicit-program" {
                if let Some(targets) = op["targets"].as_array_mut() {
                    targets.retain(|x| {
                        x.as_str()
                            .is_none_or(|id| !late.iter().any(|item| item == id))
                    });
                }
            }
        }
        ops.retain(|op| {
            op["origin"] != "implicit-program"
                || op["targets"].as_array().is_some_and(|x| !x.is_empty())
        });
        ops.push(json!({"op":"KEEP","targets":late,"origin":"post-snapshot-program","reason":"late protocol closure material; not supplied to Editor"}));
        let operations = ops.clone();
        let index = result["index"]
            .as_array_mut()
            .ok_or_else(|| CoreError::invalid("materialized index missing"))?;
        for entry in index {
            let id = required_str(entry, "unit_id")?.to_owned();
            let positions: Vec<usize> = operations
                .iter()
                .enumerate()
                .filter_map(|(i, op)| {
                    (op["target"].as_str() == Some(id.as_str())
                        || op["targets"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id.as_str()))))
                    .then_some(i)
                })
                .collect();
            entry["operation_indexes"] = json!(positions);
        }
    }
    result["program_kept_after_snapshot"] = json!(late);
    result["editor_source_snapshot_ids"] = json!(frozen_ids);
    Ok(result)
}

impl Core {
    async fn update_decision(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let expected = field_u64(p, "expected_revision")?;
        let events = p
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| CoreError::invalid("decision events must be an array"))?;
        let next = decision::apply(&agent.decision, expected, events).map_err(err)?;
        if next != agent.decision {
            let revision_id = storage::uid("decision_revision");
            storage::put_object(conn,"decision_revision",&revision_id,&json!({"chain_id":agent.chain_id,"previous_revision":expected,"value":next,"events":events})).await?;
            agent.decision = next;
            agent
                .events
                .push(json!({"type":"decision_updated","revision_id":revision_id,"time":now()}));
        }
        Ok(json!({"decision":agent.decision}))
    }
    fn update_config(&self, agent: &mut Agent, p: &Value) -> Result<Value> {
        let changes = obj(p)?;
        let allowed = [
            "prefix_layout",
            "schedule_policy",
            "task_goal",
            "task_constraints",
            "editor_prompt",
            "editor_feedback",
        ];
        if changes.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(CoreError::new(
                "unsupported",
                "configuration field is not implemented",
                501,
            ));
        }
        if changes
            .get("prefix_layout")
            .is_some_and(|v| v.as_str() != Some("shared_i"))
        {
            return Err(CoreError::invalid("lean profile requires shared_i prefix"));
        }
        if changes
            .get("schedule_policy")
            .is_some_and(|v| v.as_str() != Some("wait"))
        {
            return Err(CoreError::invalid("lean Worker must wait for Editor"));
        }
        if changes.get("task_goal").is_some_and(|v| !v.is_string())
            || changes.get("editor_prompt").is_some_and(|v| !v.is_string())
        {
            return Err(CoreError::invalid(
                "task goal and Editor prompt must be text",
            ));
        }
        if changes.get("task_constraints").is_some_and(|v| {
            !v.as_array()
                .is_some_and(|rows| rows.iter().all(Value::is_string))
        }) {
            return Err(CoreError::invalid(
                "task constraints must be an array of strings",
            ));
        }
        if let Some(policy) = changes.get("editor_feedback") {
            let policy = obj(policy)?;
            if policy
                .keys()
                .any(|key| key != "max_corrections" && key != "feedback_chars")
            {
                return Err(CoreError::invalid("unknown Editor feedback field"));
            }
            if policy
                .get("max_corrections")
                .is_some_and(|v| v.as_u64().is_none_or(|n| n > 3))
            {
                return Err(CoreError::invalid("max_corrections must be 0..3"));
            }
            if policy
                .get("feedback_chars")
                .is_some_and(|v| v.as_u64().is_none_or(|n| !(512..=32768).contains(&n)))
            {
                return Err(CoreError::invalid("feedback_chars must be 512..32768"));
            }
        }
        let current = agent
            .config
            .as_object_mut()
            .ok_or_else(|| CoreError::invalid("invalid config"))?;
        for (key, value) in changes {
            current.insert(key.clone(), value.clone());
        }
        Ok(json!({"configuration":agent.config}))
    }
    async fn publish(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
        replayed: &mut bool,
        communication: &[crate::ipc::PublishedMessage],
        new_cut_id: &str,
        board_i: Option<&Value>,
    ) -> Result<Value> {
        let key = required_str(p, "idempotency_key")?;
        let receipt_key = format!("{}:publish:{key}", agent.chain_id);
        let fingerprint = storage::digest(&json!({"chain":agent.chain_id,"payload":p}))?;
        if let Some(row) = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?")
            .bind(&receipt_key)
            .fetch_optional(&mut *conn)
            .await?
        {
            let previous: String = row.try_get("fingerprint")?;
            if previous != fingerprint {
                return Err(CoreError::conflict(
                    "idempotency key reused with different operation",
                ));
            }
            let result: String = row.try_get("result")?;
            *replayed = true;
            return Ok(serde_json::from_str(&result)?);
        }
        let expected = field_u64(p, "expected_cut")?;
        if agent.worker_claim.is_some() {
            return Err(CoreError::conflict(
                "Worker must finish before publishing a new cut",
            ));
        }
        if expected != agent.cut_seq {
            return Err(CoreError::conflict("stale publication cut"));
        }
        if agent.run_state != "running" {
            return Err(CoreError::conflict("Agent is paused or stopped"));
        }
        let id = required_str(p, "job_id")?;
        let job = agent
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::invalid("job not found"))?;
        if job.base_cut != agent.cut_seq {
            return Err(CoreError::conflict("Editor background is stale"));
        }
        if let Some(revision) = p.get("expected_candidate_revision") {
            if revision.as_u64() != Some(job.candidate_revision) {
                return Err(CoreError::conflict("candidate changed before publication"));
            }
        }
        if agent
            .level(&job.context_id, job.level)?
            .pending
            .first()
            .map(String::as_str)
            != Some(job.source_id.as_str())
        {
            return Err(CoreError::conflict("WB publication order changed"));
        }
        let current = source_snapshot(conn, agent, &job.source_id, None).await?;
        if current["open_calls"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            return Err(CoreError::new(
                "tool_group_open",
                "native tool results remain unresolved",
                409,
            ));
        }
        let mut material = combined_material(conn, agent, &job).await?;
        if let Some(board) = board_i {
            material["instant"]["board"] = board.clone();
        } else if let Some(instant) = material["instant"].as_object_mut() {
            instant.remove("board");
        }
        // IPC content arriving after the Editor snapshot is program-owned:
        // persistent content is appended with an explicit replayable origin;
        // volatile content becomes the next WB and was not seen by Editor.
        for item in communication {
            match item.message.lifecycle {
                Lifecycle::Persistent => {
                    let body = material["body"]
                        .as_str()
                        .ok_or_else(|| CoreError::invalid("materialized body missing"))?;
                    material["body"] = json!(if body.is_empty() {
                        item.message.text.clone()
                    } else {
                        format!("{body}\n{}", item.message.text)
                    });
                    material["ops"].as_array_mut().ok_or_else(||CoreError::invalid("operations missing"))?
                        .push(json!({"op":"ADD","content":item.message.text,"origin":"persistent-communication",
                                    "message_id":item.message.message_id,"receipt_id":item.receive_receipt_id}));
                }
                Lifecycle::Volatile => material["volatile"]
                    .as_array_mut()
                    .ok_or_else(|| CoreError::invalid("volatile content missing"))?
                    .push(json!(item.message.text)),
            }
        }
        if job.context_id == "main" && job.level == 1 {
            if let Some(tree) = material["instant"].get("decision_tree").cloned() {
                let next = decision::from_editor(&agent.decision, job.decision_revision, &tree)
                    .map_err(err)?;
                let rid = storage::uid("decision_revision");
                storage::put_object(conn,"decision_revision",&rid,&json!({"chain_id":agent.chain_id,"job_id":job.job_id,"previous_revision":job.decision_revision,"value":next})).await?;
                agent.decision = next;
                material["instant"]["decision_tree"] = agent.decision.clone();
            } else {
                material["instant"]["decision_tree"] = agent.decision.clone();
            }
        }
        let wb = storage::get_object(conn, &job.wb_id, Some("wb")).await?;
        let sequence = agent.level(&job.context_id, job.level)?.cbs.len() + 1;
        let cb_id = storage::uid("cb");
        let cb = json!({"cb_id":cb_id,"chain_id":agent.chain_id,"context_id":job.context_id,"level":job.level,"sequence":sequence,
                      "source_id":job.source_id,"source_ids":current["source_ids"],"wb_id":job.wb_id,"members":wb["members"],
                      "body":material["body"],"ops":material["ops"],"index":material["index"],"job_id":id,
                      "original_response_id":job.response_id,"candidate_revision":job.candidate_revision,
                      "candidate_revision_id":job.candidate_revision_id,"editor_source_snapshot_ids":material["editor_source_snapshot_ids"],
                      "program_kept_after_snapshot":material["program_kept_after_snapshot"],"communication":communication});
        storage::put_object(conn, "cb", &cb_id, &cb).await?;
        {
            let level = agent.level_mut(&job.context_id, job.level)?;
            level.cbs.push(cb_id.clone());
            level.pending.remove(0);
            level.pairs.insert(job.wb_id.clone(), cb_id.clone());
            let instant = material["instant"]
                .as_object()
                .ok_or_else(|| CoreError::invalid("invalid instantaneous state"))?;
            for (key, value) in instant {
                if value.is_null() {
                    level.instant.remove(key);
                } else {
                    level.instant.insert(key.clone(), value.clone());
                }
            }
        }
        for context in agent.contexts.values_mut() {
            for level in context.levels.values_mut() {
                if let Some(board) = board_i {
                    level.instant.insert("board".into(), board.clone());
                } else {
                    level.instant.remove("board");
                }
            }
        }
        let volatile = material["volatile"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("invalid volatile content"))?;
        let mut volatile_id = None;
        if !volatile.is_empty() {
            let messages: Vec<Value> = volatile
                .iter()
                .map(|text| json!({"role":"user","content":text}))
                .collect();
            let value = decode_source("openai-chat", &json!({"messages":messages}))?;
            let sid = storage::uid("source");
            storage::put_object(conn, "source", &sid, &value).await?;
            agent.groups.insert(sid.clone(), vec![sid.clone()]);
            agent
                .level_mut(&job.context_id, job.level)?
                .pending
                .insert(0, sid.clone());
            register_wb(
                conn,
                agent,
                &job.context_id,
                job.level,
                &sid,
                Vec::new(),
                "volatile-continuation",
            )
            .await?;
            volatile_id = Some(sid);
        }
        let cut_id = new_cut_id.to_owned();
        let next = agent.cut_seq + 1;
        let cut = json!({"cut_id":cut_id,"chain_id":agent.chain_id,"sequence":next,"previous_cut":agent.current_cut,
                       "published_level":job.level,"published_context":job.context_id,"contexts":agent.contexts,
                       "role_views":agent.role_views,"groups":agent.groups,"protected":agent.protected,"config":agent.config,"decision":agent.decision,
                       "input_epoch":agent.input_epoch,"control_epoch":agent.control_epoch,"board_id":agent.board_id});
        let cut_digest = storage::digest(&cut)?;
        let mut fixed = cut;
        fixed["digest"] = json!(cut_digest);
        storage::put_object(conn, "cut", &cut_id, &fixed).await?;
        agent.cut_seq = next;
        agent.current_cut = Some(cut_id.clone());
        let held = agent
            .jobs
            .get_mut(id)
            .ok_or_else(|| CoreError::invalid("job disappeared"))?;
        held.status = "published".into();
        held.cb_id = Some(cb_id.clone());
        for other in agent.jobs.values_mut() {
            if other.job_id != id
                && other.base_cut < agent.cut_seq
                && (other.status == "open" || other.status == "ready")
            {
                other.status = "superseded".into();
            }
        }
        agent
            .events
            .push(json!({"type":"cut_published","cut_id":cut_id,"cb_id":cb_id,"time":now()}));
        let result = json!({"cut_id":cut_id,"cb_id":cb_id,"sequence":next,"digest":cut_digest,"volatile_source_id":volatile_id,
                          "source_ids":current["source_ids"],"level":job.level,"context_id":job.context_id,"wb_id":job.wb_id});
        sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)")
            .bind(receipt_key)
            .bind(fingerprint)
            .bind(storage::json_text(&result)?)
            .execute(conn)
            .await?;
        Ok(result)
    }

    fn create_context(&self, agent: &mut Agent, p: &Value) -> Result<Value> {
        let name = required_str(p, "context_id")?;
        if name == "main" || agent.contexts.contains_key(name) {
            return Err(CoreError::conflict("context identity already exists"));
        }
        let shared = p
            .get("shared_contexts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !shared.is_empty() {
            return Err(CoreError::new(
                "unsupported",
                "shared internal-context DAG is not implemented",
                501,
            ));
        }
        let mut context = InternalContext::default();
        context.levels.insert(1, Level::default());
        agent.contexts.insert(name.into(), context);
        Ok(
            json!({"chain_id":agent.chain_id,"context_id":name,"shared_contexts":[],"agent_routes_created":0}),
        )
    }

    async fn form_higher(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let ctx = p
            .get("context_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let number = field_level(p)?;
        if number < 2 {
            return Err(CoreError::invalid("higher WB requires level >= 2"));
        }
        let members: Vec<String> = p
            .get("cb_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| CoreError::invalid("cb_ids required"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .filter(|x| !x.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| CoreError::invalid("invalid CB id"))
            })
            .collect::<Result<_>>()?;
        if members.is_empty() || members.iter().collect::<BTreeSet<_>>().len() != members.len() {
            return Err(CoreError::invalid("unique ordered CB members required"));
        }
        let lower = agent.level(ctx, number - 1)?.cbs.clone();
        let start = lower
            .iter()
            .position(|x| x == &members[0])
            .ok_or_else(|| CoreError::forbidden("member not in lower chain"))?;
        if lower.get(start..start + members.len()) != Some(members.as_slice()) {
            return Err(CoreError::conflict(
                "higher WB members must be consecutive and ordered",
            ));
        }
        let existing = agent
            .contexts
            .get(ctx)
            .and_then(|c| c.levels.get(&number))
            .map(|l| l.wbs.clone())
            .unwrap_or_default();
        let mut consumed = Vec::new();
        for wid in &existing {
            let wb = storage::get_object(conn, wid, Some("wb")).await?;
            if let Some(items) = wb["members"].as_array() {
                for item in items {
                    if let Some(id) = item.as_str() {
                        consumed.push(id.to_owned());
                    }
                }
            }
        }
        if members.iter().any(|m| consumed.contains(m)) {
            return Err(CoreError::conflict(
                "lower CB already belongs to a higher WB",
            ));
        }
        if let Some(last) = consumed.last() {
            if let Some(index) = lower.iter().position(|m| m == last) {
                if start <= index {
                    return Err(CoreError::conflict("higher WB order moved backward"));
                }
            }
        }
        let mut messages = Vec::new();
        let mut dependencies = Vec::new();
        for id in &members {
            let cb = storage::get_object(conn, id, Some("cb")).await?;
            messages.push(json!({"role":"user","content":cb["body"]}));
            for source in cb["source_ids"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("CB sources missing"))?
            {
                let source = source
                    .as_str()
                    .ok_or_else(|| CoreError::invalid("invalid CB source"))?
                    .to_owned();
                if !dependencies.contains(&source) {
                    dependencies.push(source);
                }
            }
        }
        let mut source = decode_source("openai-chat", &json!({"messages":messages}))?;
        source["dependencies"] = json!(dependencies);
        source["members"] = json!(members);
        source["level"] = json!(number);
        if let Some(units) = source["units"].as_array_mut() {
            for (index, unit) in units.iter_mut().enumerate() {
                unit["anchor"]["cb_id"] = json!(members[index]);
                unit["anchor"]["level"] = json!(number - 1);
            }
        }
        let sid = storage::uid("source");
        storage::put_object(conn, "source", &sid, &source).await?;
        agent.groups.insert(sid.clone(), vec![sid.clone()]);
        agent.level_mut(ctx, number)?.pending.push(sid.clone());
        let wid = register_wb(
            conn,
            agent,
            ctx,
            number,
            &sid,
            members.clone(),
            "ordered-lower-CB-interval",
        )
        .await?;
        Ok(
            json!({"chain_id":agent.chain_id,"context_id":ctx,"level":number,"source_id":sid,"wb_id":wid,"members":members}),
        )
    }

    async fn timeline(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        p: &Value,
    ) -> Result<Value> {
        let ctx = p
            .get("context_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let number = p
            .get("level")
            .map(|_| field_level(p))
            .transpose()?
            .unwrap_or(1);
        let kind = p.get("kind").and_then(Value::as_str).unwrap_or("wb");
        if kind != "wb" && kind != "cb" {
            return Err(CoreError::invalid("kind must be wb or cb"));
        }
        let offset = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        if limit == 0 || limit > 100 {
            return Err(CoreError::invalid("timeline limit must be 1..100"));
        }
        let level = agent.level(ctx, number)?;
        let ids = if kind == "wb" { &level.wbs } else { &level.cbs };
        let mut rows = Vec::new();
        for (index, id) in ids.iter().enumerate().skip(offset).take(limit) {
            let value = storage::get_object(conn, id, Some(kind)).await?;
            let wb = if kind == "wb" {
                Some(id.clone())
            } else {
                level
                    .pairs
                    .iter()
                    .find_map(|(w, c)| (c == id).then(|| w.clone()))
            };
            let cb = if kind == "cb" {
                Some(id.clone())
            } else {
                level.pairs.get(id).cloned()
            };
            rows.push(json!({"id":id,"context_id":ctx,"level":number,"membership_index":index,"predecessor":if index>0{Some(&ids[index-1])}else{None},
                              "successor":ids.get(index+1),"wb_id":wb,"cb_id":cb,"source_id":value["source_id"],"members":value["members"]}));
        }
        let end = offset.saturating_add(rows.len());
        Ok(
            json!({"chain_id":agent.chain_id,"context_id":ctx,"level":number,"kind":kind,
                                                            "rows":rows,"total":ids.len(),"next_offset":if end<ids.len(){Some(end)}else{None}}),
        )
    }

    async fn expand(&self, conn: &mut SqliteConnection, agent: &Agent, p: &Value) -> Result<Value> {
        let cid = p.get("cb_id").and_then(Value::as_str);
        let requested_wb = p.get("wb_id").and_then(Value::as_str);
        if cid.is_some() == requested_wb.is_some() {
            return Err(CoreError::invalid("specify exactly one cb_id or wb_id"));
        }
        for (ctx, context) in &agent.contexts {
            for (level_num, level) in &context.levels {
                let wid = if let Some(cb) = cid {
                    level
                        .pairs
                        .iter()
                        .find_map(|(w, c)| (c == cb).then(|| w.clone()))
                } else {
                    requested_wb
                        .filter(|w| level.wbs.iter().any(|id| id == w))
                        .map(str::to_owned)
                };
                if let Some(wid) = wid {
                    let wb = storage::get_object(conn, &wid, Some("wb")).await?;
                    let sid = required_str(&wb, "source_id")?;
                    let ids: Vec<String> = serde_json::from_value(wb["source_ids"].clone())?;
                    let source = source_snapshot(conn, agent, sid, Some(&ids)).await?;
                    let paired = level.pairs.get(&wid);
                    if let Some(cb) = paired {
                        storage::get_object(conn, cb, Some("cb")).await?;
                    }
                    let members = wb["members"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|m| json!({"cb_id":m,"level":level_num-1}))
                        .collect::<Vec<_>>();
                    return Ok(
                        json!({"chain_id":agent.chain_id,"context_id":ctx,"level":level_num,"wb_id":wid,"cb_id":paired,
                                 "source_ids":ids,"members":members,"original_units":source["units"]}),
                    );
                }
            }
        }
        Err(CoreError::forbidden("WB or CB is not on this chain"))
    }

    async fn selected_ids(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        role: &str,
    ) -> Result<Vec<String>> {
        let view = agent
            .role_views
            .get(role)
            .or_else(|| {
                if role.starts_with("editor:") {
                    agent.role_views.get("editor")
                } else {
                    None
                }
            })
            .ok_or_else(|| CoreError::invalid("role view not configured"))?;
        let context = agent
            .contexts
            .get(&view.context_id)
            .ok_or_else(|| CoreError::invalid("selected context missing"))?;
        if !context.shared_contexts.is_empty() {
            return Err(CoreError::new(
                "unsupported",
                "shared internal-context projection is not implemented",
                501,
            ));
        }
        let mut ids = context
            .levels
            .get(&1)
            .ok_or_else(|| CoreError::invalid("base level missing"))?
            .cbs
            .clone();
        for number in 2..=view.level {
            if let Some(level) = context.levels.get(&number) {
                for cid in &level.cbs {
                    let cb = storage::get_object(conn, cid, Some("cb")).await?;
                    let members: Vec<String> = serde_json::from_value(cb["members"].clone())?;
                    if members.is_empty() {
                        ids.push(cid.clone());
                        continue;
                    }
                    if let Some(start) = ids.iter().position(|id| id == &members[0]) {
                        if ids.get(start..start + members.len()) == Some(members.as_slice()) {
                            ids.splice(start..start + members.len(), [cid.clone()]);
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    async fn select(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        if agent.worker_claim.is_some() {
            return Err(CoreError::conflict(
                "Worker must finish before switching representation",
            ));
        }
        let ctx = p
            .get("context_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let level = field_level(p)?;
        if agent.level(ctx, level)?.cbs.is_empty() {
            return Err(CoreError::invalid("selected level has no CB"));
        }
        let roles = p
            .get("roles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![json!("worker"), json!("editor")]);
        if roles.is_empty() {
            return Err(CoreError::invalid("at least one role required"));
        }
        for role in &roles {
            let role = role
                .as_str()
                .ok_or_else(|| CoreError::invalid("invalid role"))?;
            if role != "worker" && role != "editor" && !role.starts_with("editor:") {
                return Err(CoreError::invalid("invalid role"));
            }
            agent.role_views.insert(
                role.into(),
                View {
                    context_id: ctx.into(),
                    level,
                },
            );
        }
        let previous = agent.current_cut.clone();
        if previous.is_some() {
            let id = storage::uid("cut");
            let sequence = agent.cut_seq + 1;
            let mut cut = json!({"cut_id":id,"chain_id":agent.chain_id,"sequence":sequence,"previous_cut":previous,
                               "contexts":agent.contexts,"role_views":agent.role_views,"groups":agent.groups,"protected":agent.protected,"config":agent.config,"board_id":agent.board_id,"representation_selection":true});
            cut["digest"] = json!(storage::digest(&cut)?);
            storage::put_object(conn, "cut", &id, &cut).await?;
            agent.cut_seq = sequence;
            agent.current_cut = Some(id);
        }
        let mut selected = Map::new();
        for role in &roles {
            let role = role.as_str().unwrap();
            selected.insert(
                role.into(),
                json!(self.selected_ids(conn, agent, role).await?),
            );
        }
        Ok(
            json!({"role_views":agent.role_views,"selected_cb_ids":selected,"cut_id":agent.current_cut,"sequence":agent.cut_seq,"archive_mutated":false}),
        )
    }

    async fn get_cut(&self, conn: &mut SqliteConnection, agent: &Agent) -> Result<Value> {
        let cut = if let Some(id) = &agent.current_cut {
            let archived = storage::get_object(conn, id, Some("cut")).await?;
            // A cut can reference an earlier, now-unbound board. Returning its
            // raw contexts after ACL revocation would leak old I snapshots.
            if archived["board_id"].as_str() != agent.board_id.as_deref() {
                Some(
                    json!({"cut_id":archived["cut_id"],"chain_id":archived["chain_id"],
                    "sequence":archived["sequence"],"previous_cut":archived["previous_cut"],
                    "published_level":archived["published_level"],"published_context":archived["published_context"],
                    "archived_cut_digest":archived["digest"],"projection":"metadata-only"}),
                )
            } else {
                Some(archived)
            }
        } else {
            None
        };
        let ids = self.selected_ids(conn, agent, "worker").await?;
        let mut cbs = Vec::new();
        for id in ids {
            cbs.push(storage::get_object(conn, &id, Some("cb")).await?);
        }
        let view = agent
            .role_views
            .get("worker")
            .ok_or_else(|| CoreError::invalid("Worker view missing"))?;
        let level = agent.level(&view.context_id, view.level)?;
        Ok(
            json!({"cut":cut,"cut_id":agent.current_cut,"sequence":agent.cut_seq,
                  "view":{"protected":agent.protected,"cbs":cbs,"instant":level.instant},"pending":level.pending}),
        )
    }

    async fn page(&self, conn: &mut SqliteConnection, agent: &Agent, p: &Value) -> Result<Value> {
        let kind = required_str(p, "kind")?;
        let id = required_str(p, "object_id")?;
        let allowed = match kind {
            "source" => agent.groups.values().any(|g| g.iter().any(|x| x == id)),
            "cb" | "ops" => agent
                .contexts
                .values()
                .any(|c| c.levels.values().any(|l| l.cbs.iter().any(|x| x == id))),
            "wb" => agent
                .contexts
                .values()
                .any(|c| c.levels.values().any(|l| l.wbs.iter().any(|x| x == id))),
            _ => {
                return Err(CoreError::new(
                    "unsupported",
                    "memory kind is not implemented",
                    501,
                ));
            }
        };
        if !allowed {
            return Err(CoreError::forbidden("object is not on this chain"));
        }
        let value =
            storage::get_object(conn, id, Some(if kind == "ops" { "cb" } else { kind })).await?;
        let text = if kind == "ops" {
            storage::json_text(&value["ops"])?
        } else {
            storage::json_text(&value)?
        };
        let offset = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(2048) as usize;
        if limit == 0 || limit > 8192 {
            return Err(CoreError::invalid("page limit must be 1..8192"));
        }
        let total = text.chars().count();
        let value: String = text.chars().skip(offset).take(limit).collect();
        let next = offset.saturating_add(value.chars().count());
        Ok(
            json!({"kind":kind,"object_id":id,"value":value,"offset":offset,"total":total,"next_offset":if next<total{Some(next)}else{None},"unit":"unicode-codepoints","full_digest":storage::sha256(text.as_bytes())}),
        )
    }

    async fn replay(&self, conn: &mut SqliteConnection, agent: &Agent, p: &Value) -> Result<Value> {
        let id = required_str(p, "cb_id")?;
        if !agent
            .contexts
            .values()
            .any(|c| c.levels.values().any(|l| l.cbs.iter().any(|x| x == id)))
        {
            return Err(CoreError::forbidden("CB is not on this chain"));
        }
        let cb = storage::get_object(conn, id, Some("cb")).await?;
        let wb = storage::get_object(conn, required_str(&cb, "wb_id")?, Some("wb")).await?;
        // Replay includes separately attributed late source fragments from the
        // published CB, not just the Editor's earlier frozen WB membership.
        let ids: Vec<String> = serde_json::from_value(cb["source_ids"].clone())?;
        let source =
            source_snapshot(conn, agent, required_str(&wb, "source_id")?, Some(&ids)).await?;
        let ops = cb["ops"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB operations missing"))?;
        let step = p
            .get("step")
            .and_then(Value::as_u64)
            .map(|x| x as usize)
            .unwrap_or(ops.len());
        let mut replay = editing::replay(
            &source,
            &cb["ops"],
            step,
            cb["context_id"] == "main" && cb["level"] == 1,
        )
        .map_err(err)?;
        let complete = step == ops.len();
        if complete && replay["body"] != cb["body"] {
            return Err(CoreError::new(
                "replay_integrity",
                "replayed content differs from published CB",
                409,
            ));
        }
        replay["body_matches_published_cb"] = if complete { json!(true) } else { Value::Null };
        replay["cb_id"] = json!(id);
        replay["wb_id"] = cb["wb_id"].clone();
        replay["source_ids"] = json!(ids);
        Ok(replay)
    }

    async fn revoke(&self, conn: &mut SqliteConnection, agent: &Agent, p: &Value) -> Result<Value> {
        let id = required_str(p, "source_id")?;
        if !agent.groups.values().any(|g| g.iter().any(|v| v == id)) {
            return Err(CoreError::forbidden("source is not on this chain"));
        }
        storage::get_object(conn, id, Some("source")).await?;
        sqlx::query("INSERT INTO revocations(id,record) VALUES(?,?)")
            .bind(id)
            .bind(storage::json_text(
                &json!({"source_id":id,"chain_id":agent.chain_id,"at":now()}),
            )?)
            .execute(conn)
            .await?;
        Ok(json!({"source_id":id,"access_revoked":true,"physical_erasure":false}))
    }
}

/// This is a deliberately conservative host surrogate over the entire
/// serialized request (messages and schemas), not provider-native usage.
fn surrogate_input_count(request: &Value) -> Result<provider::FrozenInputCount> {
    let text = storage::json_text(request)?;
    let measured = tiktoken_rs::o200k_base_singleton()
        .encode_ordinary(&text)
        .len() as u64;
    let tokens = measured.saturating_add(measured / 10).saturating_add(1024);
    Ok(provider::FrozenInputCount {
        tokens,
        counter_id: "o200k-complete-json-plus-10pct-and-1024-surrogate".into(),
    })
}

fn apply_profile_reasoning(profile: &Value, request: &mut Value) -> Result<()> {
    if let Some(value) = profile.get("reasoning_effort") {
        let effort = value
            .as_str()
            .filter(|effort| ["low", "medium", "high", "xhigh", "max"].contains(effort))
            .ok_or_else(|| {
                CoreError::invalid("host reasoning_effort must be low, medium, high, xhigh, or max")
            })?;
        request["reasoning_effort"] = json!(effort);
    }
    Ok(())
}

fn observed_body_value(captured: &Arc<std::sync::Mutex<Option<Vec<u8>>>>) -> Option<Value> {
    let bytes = captured.lock().ok()?.clone()?;
    if let Ok(value) = serde_json::from_slice(&bytes) {
        return Some(value);
    }
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        hex.push(char::from_digit((byte >> 4) as u32, 16)?);
        hex.push(char::from_digit((byte & 15) as u32, 16)?);
    }
    Some(json!({"invalid_json_raw_hex":hex}))
}

fn editable_editor_response(
    result: &provider::ChatCompletionResult,
    job: &Job,
    frozen_decision: &Value,
) -> std::result::Result<Value, String> {
    let calls = result
        .tool_calls
        .as_ref()
        .and_then(Value::as_array)
        .ok_or("Editor did not call sic_edit_blocks")?;
    if calls.len() != 1 || calls[0]["function"]["name"] != "sic_edit_blocks" {
        return Err("Editor must make exactly one sic_edit_blocks call and no other calls".into());
    }
    let arguments = calls[0]["function"]["arguments"]
        .as_str()
        .ok_or("Editor tool arguments are missing")?;
    let response: Value =
        serde_json::from_str(arguments).map_err(|_| "Editor tool arguments are invalid JSON")?;
    let fields = response
        .as_object()
        .ok_or("Editor response must be an object")?;
    if fields.len() != 2 || !fields.contains_key("ops") || !fields.contains_key("pvi") {
        return Err("Editor response must contain only ops and pvi".into());
    }
    let editor = job.assignments[0]["editor"]
        .as_str()
        .ok_or("Editor assignment missing")?;
    let mut responses = Map::new();
    responses.insert(editor.to_owned(), response.clone());
    let material = editing::materialize(
        &job.source_snapshot,
        &job.assignments,
        &Value::Object(responses),
        job.context_id == "main" && job.level == 1,
    )?;
    if job.context_id == "main" && job.level == 1 {
        if let Some(tree) = material["instant"].get("decision_tree") {
            decision::from_editor(frozen_decision, job.decision_revision, tree)?;
        }
    }
    Ok(response)
}

fn retryable_editor_output(error: &str) -> bool {
    error.starts_with("Editor delivery")
        || error.starts_with("tool call arguments")
        || error.starts_with("tool call name")
        || error.starts_with("tool call ID")
        || error.starts_with("tool_calls completion")
        || error.starts_with("provider response is not a complete text")
        || error.starts_with("provider returned an incomplete or invalid JSON response")
        || error.starts_with("native tool call index")
}

impl Core {
    async fn legacy_status(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        operation: &str,
        p: &Value,
    ) -> Result<Value> {
        if operation == "legacy.queue" {
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(50);
            if limit == 0 || limit > 100 {
                return Err(CoreError::invalid("legacy queue limit must be 1..100"));
            }
            let rows=sqlx::query("SELECT id,event,status FROM runtime_queue WHERE chain_id=? AND status='legacy-quarantined' ORDER BY id LIMIT ?")
                .bind(&agent.chain_id).bind(limit as i64).fetch_all(conn).await?;
            let mut items = Vec::new();
            for row in rows {
                let event: Option<String> = row.try_get("event")?;
                let event = event
                    .map(|value| serde_json::from_str::<Value>(&value))
                    .transpose()?;
                items.push(json!({"id":row.try_get::<String,_>("id")?,"status":row.try_get::<Option<String>,_>("status")?,"event":event}));
            }
            return Ok(json!({"items":items,"execution":"quarantined; no replay"}));
        }
        let rows = sqlx::query(
            "SELECT id FROM objects WHERE kind='legacy_python_upgrade_manifest' ORDER BY id",
        )
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let id: String = row.try_get("id")?;
            let manifest =
                storage::get_object(conn, &id, Some("legacy_python_upgrade_manifest")).await?;
            if manifest["target_agent"]["chain_id"] == agent.chain_id {
                return Ok(json!({"migrated":true,"manifest_id":id,
                    "target_run_state":agent.run_state,"source_archive_sha256":manifest["source_archive_sha256"],
                    "board_mappings":manifest["boards"],"tool_dispositions":manifest["tools"],
                    "runtime_queue":manifest["runtime_queue"],"sessions":manifest["sessions"]}));
            }
        }
        Ok(json!({"migrated":false}))
    }

    /// Return original provider usage per attempt. Missing or malformed usage
    /// remains unknown; this is not a price or physical-cache assertion.
    async fn accounting(&self, conn: &mut SqliteConnection, agent: &Agent) -> Result<Value> {
        let mut attempts = Vec::new();
        let mut unknown = 0u64;
        let mut prompt = 0u64;
        let mut completion = 0u64;
        for event in &agent.events {
            let (role, kind) = match event["type"].as_str() {
                Some("editor_attempt") => ("editor", "model_attempt"),
                Some("worker_attempt") => ("worker", "worker_attempt"),
                _ => continue,
            };
            let id = required_str(event, "record_id")?;
            let record = storage::get_object(conn, id, Some(kind)).await?;
            let usage = record.get("usage").cloned().unwrap_or(Value::Null);
            let input = usage.get("prompt_tokens").and_then(Value::as_u64);
            let output = usage.get("completion_tokens").and_then(Value::as_u64);
            if let (Some(i), Some(o)) = (input, output) {
                prompt = prompt
                    .checked_add(i)
                    .ok_or_else(|| CoreError::invalid("prompt usage overflow"))?;
                completion = completion
                    .checked_add(o)
                    .ok_or_else(|| CoreError::invalid("completion usage overflow"))?;
            } else {
                unknown += 1;
            }
            attempts.push(
                json!({"record_id":id,"role":role,"provider_reported_usage":usage,
                "input_counter":record["counter_id"],"input_surrogate":record["input_count"]}),
            );
        }
        let complete = !attempts.is_empty() && unknown == 0;
        Ok(
            json!({"attempts":attempts,"known_prompt_tokens":prompt,"known_completion_tokens":completion,
            "attempts_with_unknown_usage":unknown,"complete":complete,"priced_cost":Value::Null,
            "currency":Value::Null,"physical_cache_hit":"unknown"}),
        )
    }

    async fn check_board_access(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &Agent,
    ) -> Result<()> {
        if let Some(board) = &agent.board_id {
            BoardService::initialize_in_transaction(tx).await?;
            let principal = BoardActor::from_authenticated_host(&agent.chain_id)?;
            BoardService::read_latest_i_in_transaction(tx, &principal, board).await?;
        }
        Ok(())
    }

    async fn dispatch_board(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &mut Agent,
        operation: &str,
        p: &Value,
    ) -> Result<Value> {
        BoardService::initialize_in_transaction(tx).await?;
        let principal = BoardActor::from_authenticated_host(&agent.chain_id)?;
        if operation == "board.unbind" {
            agent.board_id = None;
            for context in agent.contexts.values_mut() {
                for level in context.levels.values_mut() {
                    level.instant.remove("board");
                }
            }
            return Ok(json!({"bound":false}));
        }
        if operation == "board.create" {
            if agent.board_id.is_some() {
                return Err(CoreError::conflict("Agent already has a bound board"));
            }
            let acl = parse_board_acl(p.get("acl"), &agent.chain_id)?;
            if !acl.readers().contains(&agent.chain_id) || !acl.managers().contains(&agent.chain_id)
            {
                return Err(CoreError::invalid(
                    "creating Agent must retain board read and manage grants",
                ));
            }
            let id = storage::uid("board");
            let board =
                BoardService::configure_from_host_in_transaction(tx, &id, &principal, acl).await?;
            agent.board_id = Some(id.clone());
            return Ok(json!({"board_id":id,"board":board,"bound":true}));
        }
        let id = p
            .get("board_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| agent.board_id.clone())
            .ok_or_else(|| CoreError::invalid("board_id or bound board required"))?;
        if operation == "board.bind" {
            BoardService::read_latest_i_in_transaction(tx, &principal, &id).await?;
            for context in agent.contexts.values_mut() {
                for level in context.levels.values_mut() {
                    level.instant.remove("board");
                }
            }
            agent.board_id = Some(id.clone());
            return Ok(json!({"board_id":id,"bound":true,"refresh":"next CB publication"}));
        }
        match operation {
            "board.get" => BoardService::read_latest_i_in_transaction(tx, &principal, &id).await,
            "board.history" => Ok(
                json!({"events":BoardService::read_history_in_transaction(tx,&principal,&id,
                p.get("after_revision").and_then(Value::as_u64).unwrap_or(0),p.get("limit").and_then(Value::as_u64).unwrap_or(50)).await?}),
            ),
            "board.acl.update" => {
                BoardService::update_acl_from_host_in_transaction(
                    tx,
                    &principal,
                    &id,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    parse_board_acl(p.get("acl"), &agent.chain_id)?,
                )
                .await
            }
            "board.entry.text" => {
                BoardService::publish_text_in_transaction(
                    tx,
                    &principal,
                    &id,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    required_str(p, "text")?,
                )
                .await
            }
            "board.entry.data" => {
                BoardService::publish_structured_in_transaction(
                    tx,
                    &principal,
                    &id,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    p.get("content")
                        .ok_or_else(|| CoreError::invalid("board content required"))?,
                )
                .await
            }
            "board.task.create" => {
                BoardService::create_task_in_transaction(
                    tx,
                    &principal,
                    &id,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    required_str(p, "title")?,
                    p.get("detail").unwrap_or(&Value::Null),
                )
                .await
            }
            "board.task.claim" => {
                BoardService::claim_task_in_transaction(
                    tx,
                    &principal,
                    &id,
                    required_str(p, "task_id")?,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                )
                .await
            }
            "board.task.release" => {
                BoardService::release_task_in_transaction(
                    tx,
                    &principal,
                    &id,
                    required_str(p, "task_id")?,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                )
                .await
            }
            "board.task.progress" => {
                let status: TaskStatus = serde_json::from_value(
                    p.get("status")
                        .cloned()
                        .ok_or_else(|| CoreError::invalid("task status required"))?,
                )?;
                BoardService::update_own_task_in_transaction(
                    tx,
                    &principal,
                    &id,
                    required_str(p, "task_id")?,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    status,
                )
                .await
            }
            "board.task.transition" => {
                let status: TaskStatus = serde_json::from_value(
                    p.get("status")
                        .cloned()
                        .ok_or_else(|| CoreError::invalid("task status required"))?,
                )?;
                BoardService::transition_task_in_transaction(
                    tx,
                    &principal,
                    &id,
                    required_str(p, "task_id")?,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "idempotency_key")?,
                    status,
                )
                .await
            }
            _ => Err(CoreError::new(
                "unsupported_operation",
                "board operation is not implemented",
                501,
            )),
        }
    }

    async fn dispatch_model_builtin(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &mut Agent,
        context: &OperationContext,
        name: &str,
        args: &Value,
        call_identity: &str,
    ) -> Result<Value> {
        let fields = obj(args)?;
        let unique = format!("model:{call_identity}");
        let mut ignored = false;
        match name {
            "sic_socket_read" => {
                if fields.keys().any(|key| {
                    !["socket_id", "checkpoint_id", "offset", "limit"].contains(&key.as_str())
                }) {
                    return Err(CoreError::invalid("unknown Socket read argument"));
                }
                let socket = required_str(args, "socket_id")?;
                let checkpoint = required_str(args, "checkpoint_id")?;
                self.ensure_ipc_route(tx, agent, &context.actor).await?;
                self.ipc_store()?
                    .transact_in_transaction(tx, |state| {
                        let endpoint = state
                            .socket(socket)
                            .ok_or_else(|| CoreError::invalid("Socket not found"))?;
                        if endpoint.checkpoint_id() != checkpoint {
                            return Err(CoreError::forbidden(
                                "Socket checkpoint differs from requested checkpoint",
                            ));
                        }
                        Ok(())
                    })
                    .await?;
                self.dispatch_ipc(tx,agent,context,"socket.read",&json!({"socket_id":socket,
                    "offset":args.get("offset").cloned().unwrap_or(json!(0)),"limit":args.get("limit").cloned().unwrap_or(json!(2048))}),&mut ignored).await
            }
            "sic_socket_spawn" => {
                if fields
                    .keys()
                    .any(|key| !["socket_id", "text", "lifecycle"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown socket spawn argument"));
                }
                self.dispatch_ipc(tx,agent,context,"socket.spawn",&json!({"socket_id":required_str(args,"socket_id")?,
                    "text":required_str(args,"text")?,"lifecycle":args["lifecycle"],"request_id":unique,"message_id":unique}),&mut ignored).await
            }
            "sic_message_send" => {
                if fields
                    .keys()
                    .any(|key| !["target", "text", "lifecycle"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown message argument"));
                }
                self.dispatch_ipc(tx,agent,context,"socket.message",&json!({"socket_id":required_str(args,"target")?,
                    "text":required_str(args,"text")?,"lifecycle":args["lifecycle"],"request_id":unique,"message_id":unique}),&mut ignored).await
            }
            "sic_signal_send" => {
                if fields
                    .keys()
                    .any(|key| !["target", "signal", "payload"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown Signal argument"));
                }
                if args
                    .get("payload")
                    .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|v| !v.is_empty()))
                {
                    return Err(CoreError::new(
                        "unsupported",
                        "Signal payload is not yet supported",
                        501,
                    ));
                }
                self.dispatch_ipc(
                    tx,
                    agent,
                    context,
                    "socket.signal",
                    &json!({"socket_id":required_str(args,"target")?,
                    "signal_id":unique,"action":required_str(args,"signal")?}),
                    &mut ignored,
                )
                .await
            }
            "sic_message_receive" => {
                if fields.keys().any(|key| key != "cursor") {
                    return Err(CoreError::invalid("unknown Message receive argument"));
                }
                let cursor = match args.get("cursor") {
                    None => 0,
                    Some(value) => value
                        .as_str()
                        .ok_or_else(|| CoreError::invalid("cursor must be text"))?
                        .parse::<u64>()
                        .map_err(|_| CoreError::invalid("invalid receive cursor"))?,
                };
                self.ensure_ipc_route(tx, agent, &context.actor).await?;
                self.ipc_store()?
                    .transact_in_transaction(tx, |state| {
                        let route = state
                            .route(&agent.chain_id)
                            .ok_or_else(|| CoreError::invalid("IPC route missing"))?;
                        let rows: Vec<Value> = route
                            .inbox()
                            .iter()
                            .filter(|entry| {
                                entry.sequence > cursor && entry.consumed_by_cut.is_some()
                            })
                            .take(100)
                            .map(|entry| json!(entry))
                            .collect();
                        let next = rows
                            .last()
                            .and_then(|row| row["sequence"].as_u64())
                            .unwrap_or(cursor);
                        Ok(json!({"messages":rows,"next_cursor":next.to_string()}))
                    })
                    .await
            }
            "sic_signal_wait" => {
                if fields
                    .keys()
                    .any(|key| !["signal_id", "timeout_ms"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown Signal wait argument"));
                }
                if args
                    .get("timeout_ms")
                    .is_some_and(|v| v.as_u64() != Some(0))
                {
                    return Err(CoreError::new(
                        "unsupported",
                        "Signal blocking wait is not yet implemented; poll with timeout_ms=0",
                        501,
                    ));
                }
                let id = required_str(args, "signal_id")?;
                self.ensure_ipc_route(tx, agent, &context.actor).await?;
                self.ipc_store()?.transact_in_transaction(tx,|state| {
                    let route=state.route(&agent.chain_id).ok_or_else(||CoreError::invalid("IPC route missing"))?;
                    Ok(json!({"signal_id":id,"receipt":route.signal_receipts().get(id),"status":route.status()}))
                }).await
            }
            "sic_decision_read" => {
                if fields.keys().any(|key| key != "view") {
                    return Err(CoreError::invalid("unknown decision read argument"));
                }
                let view = required_str(args, "view")?;
                let rows = agent.decision["views"]
                    .get(view)
                    .ok_or_else(|| CoreError::invalid("unknown decision view"))?;
                Ok(json!({"view":view,"items":rows,"revision":agent.decision["revision"]}))
            }
            "sic_decision_write" => {
                if fields.keys().any(|key| key != "events") {
                    return Err(CoreError::invalid("unknown decision write argument"));
                }
                let events = args
                    .get("events")
                    .and_then(Value::as_array)
                    .filter(|rows| rows.len() <= 64)
                    .ok_or_else(|| {
                        CoreError::invalid("decision events require at most 64 entries")
                    })?;
                if events.iter().any(|event| event["kind"] == "complete") {
                    return Err(CoreError::forbidden(
                        "only the host may attest decision completion",
                    ));
                }
                let revision = agent.decision["revision"]
                    .as_u64()
                    .ok_or_else(|| CoreError::invalid("decision revision missing"))?;
                self.update_decision(
                    tx,
                    agent,
                    &json!({"events":events,"expected_revision":revision}),
                )
                .await
            }
            "sic_board_read" => {
                if !fields.is_empty() {
                    return Err(CoreError::invalid("board read accepts no arguments"));
                }
                self.dispatch_board(tx, agent, "board.get", &json!({}))
                    .await
            }
            "sic_board_post" => {
                if fields
                    .keys()
                    .any(|key| !["text", "expected_revision"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown board post argument"));
                }
                self.dispatch_board(tx,agent,"board.entry.text",&json!({"text":required_str(args,"text")?,
                    "expected_revision":field_u64(args,"expected_revision")?,"idempotency_key":unique})).await
            }
            "sic_board_claim" | "sic_board_release" => {
                if fields
                    .keys()
                    .any(|key| !["task_id", "expected_revision"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown board task argument"));
                }
                let operation = if name == "sic_board_claim" {
                    "board.task.claim"
                } else {
                    "board.task.release"
                };
                self.dispatch_board(tx,agent,operation,&json!({"task_id":required_str(args,"task_id")?,
                    "expected_revision":field_u64(args,"expected_revision")?,"idempotency_key":unique})).await
            }
            "sic_board_progress" => {
                if fields
                    .keys()
                    .any(|key| !["task_id", "expected_revision", "status"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown board progress argument"));
                }
                self.dispatch_board(tx,agent,"board.task.progress",&json!({"task_id":required_str(args,"task_id")?,
                    "expected_revision":field_u64(args,"expected_revision")?,"status":required_str(args,"status")?,"idempotency_key":unique})).await
            }
            "sic_memory_page" => {
                if fields
                    .keys()
                    .any(|key| !["kind", "object_id", "offset", "limit"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown memory page argument"));
                }
                for key in ["offset", "limit"] {
                    if args.get(key).is_some_and(|v| v.as_u64().is_none()) {
                        return Err(CoreError::invalid(
                            "memory page bounds must be nonnegative integers",
                        ));
                    }
                }
                self.page(tx, agent, args).await
            }
            "sic_memory_replay" => {
                if fields
                    .keys()
                    .any(|key| !["cb_id", "step"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown replay argument"));
                }
                if args.get("step").is_some_and(|v| v.as_u64().is_none()) {
                    return Err(CoreError::invalid(
                        "replay step must be a nonnegative integer",
                    ));
                }
                self.replay(tx, agent, args).await
            }
            "sic_hierarchy_timeline" => {
                if fields.keys().any(|key| {
                    !["context_id", "level", "kind", "offset", "limit"].contains(&key.as_str())
                }) {
                    return Err(CoreError::invalid("unknown hierarchy timeline argument"));
                }
                for key in ["level", "offset", "limit"] {
                    if args.get(key).is_some_and(|v| v.as_u64().is_none()) {
                        return Err(CoreError::invalid(
                            "timeline bounds must be nonnegative integers",
                        ));
                    }
                }
                self.timeline(tx, agent, args).await
            }
            "sic_hierarchy_expand" => {
                if fields
                    .keys()
                    .any(|key| !["cb_id", "wb_id"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown hierarchy expand argument"));
                }
                self.expand(tx, agent, args).await
            }
            _ => Err(CoreError::new(
                "unsupported",
                "model tool is not implemented",
                501,
            )),
        }
    }

    /// One-shot, durable admission followed by a bounded Editor/hierarchy pass
    /// on the claimed child. The running receipt commits before Provider I/O;
    /// any crash therefore leaves a non-replayable running task.
    async fn prepare_peer_task(&self, context: &OperationContext, p: &Value) -> Result<Value> {
        let fields = obj(p)?;
        if fields.keys().any(|key| {
            ![
                "task_id",
                "claim_id",
                "expected_revision",
                "prepare_id",
                "policy",
            ]
            .contains(&key.as_str())
        }) {
            return Err(CoreError::invalid("unknown peer task prepare field"));
        }
        let chain_id = context
            .chain_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                CoreError::invalid("peer task prepare requires a requester or peer chain")
            })?;
        if !context.allow_write || !context.allow_model {
            return Err(CoreError::forbidden(
                "peer task prepare requires host write and model authorization",
            ));
        }
        let policy_value = p
            .get("policy")
            .cloned()
            .ok_or_else(|| CoreError::invalid("bounded scheduler policy is required"))?;
        let policy: crate::scheduler::SchedulerPolicy = serde_json::from_value(policy_value)?;
        validate_peer_prepare_policy(&policy)?;
        let normalized_policy = as_value(&policy)?;
        let task_id = required_str(p, "task_id")?;
        let claim_id = required_str(p, "claim_id")?;
        let prepare_id = required_str(p, "prepare_id")?;
        let expected_revision = field_u64(p, "expected_revision")?;

        let (admission, worker_context_id, worker_level) = {
            let mut tx = self.store.begin().await?;
            let parent_or_child = load_agent(&mut tx, chain_id, &context.actor).await?;
            check_dependencies(&mut tx, &parent_or_child).await?;
            self.check_board_access(&mut tx, &parent_or_child).await?;
            let admission = crate::peer_tasks::begin_prepare_in_transaction(
                &mut tx,
                &context.actor,
                chain_id,
                task_id,
                claim_id,
                prepare_id,
                expected_revision,
                &normalized_policy,
            )
            .await?;
            if !admission.newly_started {
                tx.commit().await?;
                return Ok(peer_prepare_replay(&admission, task_id, prepare_id));
            }
            let child_chain_id = required_str(&admission.task, "peer_chain_id")?;
            let child = load_agent(&mut tx, child_chain_id, &context.actor).await?;
            check_dependencies(&mut tx, &child).await?;
            self.check_board_access(&mut tx, &child).await?;
            if child.run_state != "running" {
                return Err(CoreError::conflict("peer Agent is not running"));
            }
            let worker_view = child
                .role_views
                .get("worker")
                .ok_or_else(|| CoreError::invalid("child Worker view is missing"))?;
            if worker_view.context_id != policy.context_id {
                return Err(CoreError::conflict(
                    "prepare context differs from the peer Worker context",
                ));
            }
            let values = (admission, worker_view.context_id.clone(), worker_view.level);
            tx.commit().await?;
            values
        };

        let child_chain_id = required_str(&admission.task, "peer_chain_id")?.to_owned();
        let child_context = OperationContext {
            actor: context.actor.clone(),
            chain_id: Some(child_chain_id.clone()),
            allow_write: context.allow_write,
            allow_model: context.allow_model,
            allow_revoke: context.allow_revoke,
        };
        let scheduled =
            Box::pin(self.operate("runtime.schedule", &child_context, &normalized_policy)).await;
        let (report, invocation_error) = match scheduled {
            Ok(value) => (value.get("result").cloned().unwrap_or(Value::Null), None),
            Err(error) => (
                Value::Null,
                Some(json!({"code":error.code,"status":error.status,"message":error.message})),
            ),
        };
        let delivery = if report.is_object() {
            self.verify_peer_task_message_delivery(
                &child_chain_id,
                &admission.task,
                &worker_context_id,
                worker_level,
                &report,
            )
            .await
            .unwrap_or_else(|error| json!({"verified":false,"verification_error":error.message}))
        } else {
            json!({"verified":false,"reason":"scheduler invocation returned no report"})
        };
        let provider_called = report["model_calls_observed"].as_u64().unwrap_or(0) > 0;
        let scheduler_completed =
            report["completed"].as_bool().unwrap_or(false) && report["failure"].is_null();
        let delivery_verified = delivery["verified"].as_bool().unwrap_or(false);
        let prepared = invocation_error.is_none()
            && scheduler_completed
            && delivery_verified
            && provider_called;
        let final_state = if prepared { "prepared" } else { "unknown" };
        let unknown_reason = (!prepared).then(|| {
            if let Some(error) = &invocation_error {
                format!(
                    "runtime.schedule failed: {}",
                    error["code"].as_str().unwrap_or("unknown")
                )
            } else if !scheduler_completed {
                "bounded Editor/hierarchy scheduler did not complete".to_owned()
            } else if !delivery_verified {
                "spawn Message was not verified in this run's eligible CB publication".to_owned()
            } else {
                "Provider Editor execution was not observed".to_owned()
            }
        });
        let result = json!({
            "task_id":task_id,
            "prepare_id":prepare_id,
            "peer_chain_id":child_chain_id,
            "state":final_state,
            "ready_for_dsh":prepared,
            "replayed":false,
            "execution_started":true,
            "evidence":{
                "provider_called":provider_called,
                "provider_calls_unknown":report["model_calls_reserved_unknown"].as_u64().unwrap_or(0),
                "spawn_message_delivery":delivery,
                "scheduler":report,
                "invocation_error":invocation_error,
                "worker_run":false,
                "external_tools_executed":false,
                "requester_message_delivered":false,
                "physical_cache_hit":"unknown"
            }
        });
        let mut tx = self.store.begin().await?;
        let finalized = crate::peer_tasks::finish_prepare_in_transaction(
            &mut tx,
            &context.actor,
            chain_id,
            task_id,
            claim_id,
            prepare_id,
            admission.running_revision,
            final_state,
            unknown_reason.as_deref(),
            &result,
        )
        .await?;
        if finalized["state"] == "prepared" {
            let mut peer = load_agent(&mut tx, &child_chain_id, &context.actor).await?;
            peer.events.push(json!({
                "type":"peer_task_prepared",
                "task_id":task_id,
                "prepare_id":prepare_id,
                "time":now()
            }));
            save_agent(&mut tx, &peer).await?;
        }
        tx.commit().await?;
        Ok(json!({
            "ok":true,
            "operation":"peer.task.prepare",
            "context":{"actor":context.actor,"requester_or_peer_chain_id":chain_id},
            "evidence":{"provider_called":finalized["evidence"]["provider_called"],"physical_cache_hit":"unknown"},
            "result":finalized
        }))
    }

    async fn verify_peer_task_message_delivery(
        &self,
        child_chain_id: &str,
        task: &Value,
        worker_context_id: &str,
        worker_level: u32,
        report: &Value,
    ) -> Result<Value> {
        let ipc = self.ipc_store()?.load().await?;
        let route = ipc.route(child_chain_id).ok_or_else(|| {
            CoreError::new("peer_message_unverified", "child IPC route is missing", 409)
        })?;
        let message_id = required_str(task, "message_id")?;
        let entry = route
            .inbox()
            .iter()
            .find(|entry| entry.message.message_id == message_id)
            .ok_or_else(|| {
                CoreError::new(
                    "peer_message_unverified",
                    "spawn Message is missing from child inbox",
                    409,
                )
            })?;
        let consumed_cut = entry.consumed_by_cut.as_deref().ok_or_else(|| {
            CoreError::new(
                "peer_message_unverified",
                "spawn Message remains unconsumed",
                409,
            )
        })?;
        let receipt = route
            .publication_receipts()
            .get(consumed_cut)
            .ok_or_else(|| {
                CoreError::new(
                    "peer_message_unverified",
                    "spawn Message has no publication receipt",
                    409,
                )
            })?;
        if !receipt.message_ids.iter().any(|id| id == message_id) {
            return Err(CoreError::new(
                "peer_message_unverified",
                "publication receipt omits the spawn Message",
                409,
            ));
        }
        let published = report["published"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("scheduler publication list is missing"))?;
        let eligible = |block: &&Value| {
            block["cb_id"].as_str() == Some(receipt.cb_id.as_str())
                && block["level"]
                    .as_u64()
                    .is_some_and(|level| level > 0 && level <= worker_level as u64)
                && worker_context_id == report["context_id"].as_str().unwrap_or_default()
        };
        let consume_block = published.iter().filter(eligible).next().ok_or_else(|| {
            CoreError::new(
                "peer_message_unverified",
                "no eligible CB from this scheduler run consumed the spawn Message",
                409,
            )
        })?;
        let lifecycle = task["lifecycle"].as_str().unwrap_or("");
        let (delivery_cb_id, provider_called) = match lifecycle {
            "P" => {
                if !receipt
                    .persistent_in_cb
                    .iter()
                    .any(|item| item.message.message_id == message_id)
                {
                    return Err(CoreError::new(
                        "peer_message_unverified",
                        "persistent spawn Message is not recorded in the eligible CB receipt",
                        409,
                    ));
                }
                (
                    receipt.cb_id.clone(),
                    consume_block["provider_called"].as_bool().unwrap_or(false),
                )
            }
            "V" => {
                if !receipt
                    .volatile_next_wb
                    .iter()
                    .any(|item| item.message.message_id == message_id)
                {
                    return Err(CoreError::new(
                        "peer_message_unverified",
                        "volatile spawn Message is not recorded in the eligible CB receipt",
                        409,
                    ));
                }
                let source = consume_block["volatile_source_id"]
                    .as_str()
                    .ok_or_else(|| {
                        CoreError::new(
                            "peer_message_unverified",
                            "volatile continuation source was not created",
                            409,
                        )
                    })?;
                let next = published
                    .iter()
                    .find(|block| {
                        block["cb_id"] != receipt.cb_id
                            && block["level"]
                                .as_u64()
                                .is_some_and(|level| level > 0 && level <= worker_level as u64)
                            && block["source_ids"].as_array().is_some_and(|sources| {
                                sources.iter().any(|item| item.as_str() == Some(source))
                            })
                            && worker_context_id
                                == report["context_id"].as_str().unwrap_or_default()
                    })
                    .ok_or_else(|| {
                        CoreError::new(
                            "peer_message_unverified",
                            "volatile source was not processed by a subsequent eligible Editor",
                            409,
                        )
                    })?;
                (
                    next["cb_id"].as_str().unwrap_or_default().to_owned(),
                    next["provider_called"].as_bool().unwrap_or(false),
                )
            }
            _ => return Err(CoreError::invalid("peer task lifecycle is invalid")),
        };
        if !provider_called {
            return Err(CoreError::new(
                "peer_message_unverified",
                "eligible CB was not produced by a Provider Editor call",
                409,
            ));
        }
        Ok(json!({
            "verified":true,
            "message_id":message_id,
            "lifecycle":lifecycle,
            "content_retained":if lifecycle == "P" {
                json!({"status":"verified","basis":"persistent Message receipt"})
            } else {
                json!({"status":"unknown","basis":"volatile source reached a subsequent Editor input; output content may change"})
            },
            "consumed_cut_id":consumed_cut,
            "consume_receipt_id":receipt.receipt_id,
            "consume_cb_id":receipt.cb_id,
            "delivery_cb_id":delivery_cb_id,
            "provider_editor_call_observed":true,
            "worker_ran":false,
            "external_tool_executed":false
        }))
    }

    async fn reply_peer_task_in_transaction(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        context: &OperationContext,
        caller_chain_id: &str,
        payload: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        let fields = obj(payload)?;
        if fields.keys().any(|key| {
            !["task_id", "reply_id", "expected_attempt_id", "socket_id"].contains(&key.as_str())
        }) {
            return Err(CoreError::invalid("unknown peer task reply field"));
        }
        let task_id = required_str(payload, "task_id")?;
        let reply_id = required_str(payload, "reply_id")?;
        let expected_attempt_id = required_str(payload, "expected_attempt_id")?;
        let socket_id = required_str(payload, "socket_id")?.to_owned();
        if reply_id.trim().is_empty() || reply_id.len() > 128 {
            return Err(CoreError::invalid(
                "reply_id must be nonempty and at most 128 bytes",
            ));
        }
        let task = crate::peer_tasks::get_scoped_in_transaction(
            tx,
            &context.actor,
            caller_chain_id,
            task_id,
        )
        .await?;
        if task["state"] != "prepared" {
            return Err(CoreError::conflict(
                "peer task must have a verified prepare receipt before reply",
            ));
        }
        let requester_chain_id = required_str(&task, "requester_chain_id")?.to_owned();
        let peer_chain_id = required_str(&task, "peer_chain_id")?.to_owned();
        let requester = load_agent(tx, &requester_chain_id, &context.actor).await?;
        check_dependencies(tx, &requester).await?;
        self.check_board_access(tx, &requester).await?;
        let peer = load_agent(tx, &peer_chain_id, &context.actor).await?;
        check_dependencies(tx, &peer).await?;
        self.check_board_access(tx, &peer).await?;

        let prepare_row = sqlx::query(
            "SELECT prepare_id,state,result FROM sic_peer_task_prepares WHERE task_id=? AND actor=?",
        )
        .bind(task_id)
        .bind(&context.actor)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| CoreError::conflict("peer task has no durable prepare receipt"))?;
        let prepare_id: String = prepare_row.try_get("prepare_id")?;
        let prepare_state: String = prepare_row.try_get("state")?;
        let prepare_encoded: String = prepare_row.try_get("result")?;
        if prepare_state != "prepared" {
            return Err(CoreError::conflict("peer prepare outcome is not verified"));
        }
        let prepare: Value = serde_json::from_str(&prepare_encoded)?;
        if prepare["ready_for_dsh"] != true
            || prepare["state"] != "prepared"
            || prepare["evidence"]["spawn_message_delivery"]["verified"] != true
        {
            return Err(CoreError::conflict("peer prepare evidence is incomplete"));
        }
        let delivery = &prepare["evidence"]["spawn_message_delivery"];
        let delivery_cb_id = required_str(delivery, "delivery_cb_id")?;
        let task_message_id = required_str(&task, "message_id")?;
        if required_str(delivery, "message_id")? != task_message_id {
            return Err(CoreError::conflict(
                "prepare receipt does not match the spawned task Message",
            ));
        }

        let fingerprint = storage::digest(&json!({
            "task_id":task_id,"reply_id":reply_id,"attempt_id":expected_attempt_id,
            "socket_id":socket_id
        }))?;
        self.ipc_store()?
            .transact_in_transaction(tx, |state| {
                let socket = state.socket(&socket_id).ok_or_else(|| {
                    CoreError::new("not_found", "peer reply Socket is missing", 404)
                })?;
                if socket.owner_route_id() != requester_chain_id
                    || !(socket
                        .acl()
                        .message
                        .iter()
                        .any(|grant| grant == &peer_chain_id)
                        || socket
                            .acl()
                            .message
                            .iter()
                            .any(|grant| grant == &context.actor))
                {
                    return Err(CoreError::forbidden(
                        "requester Socket does not currently authorize this peer to send Messages",
                    ));
                }
                Ok(())
            })
            .await?;
        if let Some(row) = sqlx::query("SELECT reply_id,fingerprint,result FROM sic_peer_task_replies WHERE task_id=? AND attempt_id=?")
            .bind(task_id).bind(expected_attempt_id).fetch_optional(&mut **tx).await?
        {
            let old_reply_id: String = row.try_get("reply_id")?;
            let old_fingerprint: String = row.try_get("fingerprint")?;
            if old_reply_id != reply_id || old_fingerprint != fingerprint {
                return Err(CoreError::conflict(
                    "this Worker attempt already has a different peer reply",
                ));
            }
            let encoded: String = row.try_get("result")?;
            let mut receipt: Value = serde_json::from_str(&encoded)?;
            receipt["replayed"] = json!(true);
            *replayed = true;
            return Ok(receipt);
        }
        if sqlx::query("SELECT 1 FROM sic_peer_task_replies WHERE task_id=? AND reply_id=?")
            .bind(task_id)
            .bind(reply_id)
            .fetch_optional(&mut **tx)
            .await?
            .is_some()
        {
            return Err(CoreError::conflict("peer reply id was already used"));
        }

        let (attempt_index, attempt_event) = peer
            .events
            .iter()
            .enumerate()
            .rev()
            .find(|(_, event)| event["type"] == "worker_attempt")
            .ok_or_else(|| CoreError::conflict("peer has no Worker attempt after prepare"))?;
        let attempt_id = required_str(attempt_event, "record_id")?;
        if attempt_id != expected_attempt_id {
            return Err(CoreError::conflict(
                "expected attempt is not the peer's most recent Worker result",
            ));
        }
        let prepared_event_index = peer
            .events
            .iter()
            .enumerate()
            .rev()
            .find(|(_, event)| {
                event["type"] == "peer_task_prepared"
                    && event["task_id"] == task_id
                    && event["prepare_id"] == prepare_id
            })
            .map(|(index, _)| index)
            .ok_or_else(|| CoreError::conflict("peer prepare event is missing"))?;
        if attempt_index <= prepared_event_index {
            return Err(CoreError::conflict(
                "Worker result predates the completed peer prepare",
            ));
        }
        let worker_run_id = required_str(attempt_event, "worker_run_id")?;
        let source_id = required_str(attempt_event, "source_id")?;
        let worker_started = peer.events.iter().enumerate().any(|(index, event)| {
            index > prepared_event_index
                && index < attempt_index
                && event["type"] == "worker_run_started"
                && event["record_id"] == worker_run_id
        });
        if !worker_started {
            return Err(CoreError::conflict(
                "Worker result has no post-prepare run receipt",
            ));
        }
        let attempt = storage::get_object(tx, attempt_id, Some("worker_attempt")).await?;
        if attempt["source_id"] != source_id
            || attempt["worker_run_id"] != worker_run_id
            || !attempt["error"].is_null()
            || attempt["stale"] == true
            || attempt["interrupted"] == true
            || attempt["input_changed"] == true
            || attempt["dependencies_revoked"] == true
        {
            return Err(CoreError::conflict(
                "most recent Worker result is not safely adoptable",
            ));
        }
        if peer.worker_claim.is_some()
            || peer.run_state != "running"
            || attempt["cut_seq"].as_u64() != Some(peer.cut_seq)
            || attempt["result_input_epoch"].as_u64() != Some(peer.input_epoch)
            || attempt["result_control_epoch"].as_u64() != Some(peer.control_epoch)
            || peer.jobs.values().any(|job| job.status == "inflight")
        {
            return Err(CoreError::conflict(
                "peer Worker result is stale or the peer is still running",
            ));
        }
        let run = storage::get_object(tx, worker_run_id, Some("worker_run")).await?;
        let run_cb_ids = run["plan"]["data"]["cb_ids"]
            .as_array()
            .ok_or_else(|| CoreError::conflict("Worker run has no frozen CB boundary"))?;
        if !run_cb_ids
            .iter()
            .any(|id| id.as_str() == Some(delivery_cb_id))
        {
            return Err(CoreError::conflict(
                "Worker input did not include the CB carrying the peer task Message",
            ));
        }
        let raw_message = &attempt["raw_response"]["choices"][0]["message"];
        let text = raw_message["content"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| CoreError::conflict("Worker result has no plain-text reply"))?;
        if raw_message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
        {
            return Err(CoreError::conflict(
                "Worker result still requests tools and cannot be replied yet",
            ));
        }
        if text.len() > 65_536 {
            return Err(CoreError::conflict(
                "Worker reply exceeds the message limit",
            ));
        }
        let source = storage::get_object(tx, source_id, Some("source")).await?;
        if !peer
            .groups
            .values()
            .any(|sources| sources.iter().any(|id| id == source_id))
            || source["open_calls"]
                .as_array()
                .is_some_and(|calls| !calls.is_empty())
        {
            return Err(CoreError::conflict(
                "Worker source is not adopted or still has open tool calls",
            ));
        }
        let source_messages = source["sidecar"]["messages"]
            .as_array()
            .ok_or_else(|| CoreError::conflict("Worker source has no preserved response"))?;
        if !source_messages.iter().any(|message| {
            message["role"] == "assistant"
                && message["content"] == text
                && message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
        }) {
            return Err(CoreError::conflict(
                "Worker source does not match the preserved Provider response",
            ));
        }

        let mut pending_tools = false;
        for row in sqlx::query("SELECT state FROM host_tool_tasks")
            .fetch_all(&mut **tx)
            .await?
        {
            let encoded: String = row.try_get("state")?;
            let host_task: Value = serde_json::from_str(&encoded)?;
            if host_task["chain_id"] == peer_chain_id
                && host_task["actor"] == context.actor
                && !matches!(
                    host_task["status"].as_str(),
                    Some("completed" | "delivered_unattested")
                )
            {
                pending_tools = true;
                break;
            }
        }
        if pending_tools {
            return Err(CoreError::conflict(
                "peer has unresolved external tool tasks; reply is withheld",
            ));
        }
        if sqlx::query("SELECT 1 FROM sic_dynamic_toolsets WHERE chain_id=? AND actor=?")
            .bind(&peer_chain_id)
            .bind(&context.actor)
            .fetch_optional(&mut **tx)
            .await?
            .is_some()
            && !ToolStore::active_leases_in_transaction(tx, &peer_chain_id, &context.actor)
                .await?
                .is_empty()
        {
            return Err(CoreError::conflict(
                "peer has an in-flight dynamic tool lease; reply is withheld",
            ));
        }

        let stable_key = storage::digest(&json!({"task_id":task_id,"reply_id":reply_id}))?;
        let request_id = format!("sic-peer-reply:{stable_key}");
        let outbound_message_id = format!("sic-peer-reply-message:{stable_key}");
        let task_socket_id = socket_id.clone();
        let requester_id = requester_chain_id.clone();
        let peer_id = peer_chain_id.clone();
        let actor = context.actor.clone();
        let task_identity = task_id.to_owned();
        let ipc = self.ipc_store()?;
        let receipt = ipc
            .transact_in_transaction(tx, |state| {
                let route = state
                    .route(&peer_id)
                    .ok_or_else(|| CoreError::new("ipc_rejected", "peer route is missing", 403))?;
                let inbox = route
                    .inbox()
                    .iter()
                    .find(|entry| entry.message.message_id == task_message_id)
                    .ok_or_else(|| CoreError::conflict("spawn Message receipt is missing"))?;
                if inbox.message.sender_route_id != requester_id
                    || inbox.message.recipient_route_id != peer_id
                    || inbox.consumed_by_cut.is_none()
                    || route
                        .message_receipts()
                        .get(task_message_id)
                        .is_none_or(|receipt| receipt.receipt_id != inbox.receive_receipt_id)
                {
                    return Err(CoreError::conflict(
                        "spawn Message receipt is not consumed by the prepared peer",
                    ));
                }
                let consumed_cut = inbox.consumed_by_cut.as_deref().unwrap_or_default();
                if route
                    .publication_receipts()
                    .get(consumed_cut)
                    .is_none_or(|receipt| {
                        receipt.cb_id
                            != prepare["evidence"]["spawn_message_delivery"]["consume_cb_id"]
                            || !receipt.message_ids.iter().any(|id| id == task_message_id)
                    })
                {
                    return Err(CoreError::conflict(
                        "spawn Message has no matching CB publication receipt",
                    ));
                }
                let socket = state.socket(&task_socket_id).ok_or_else(|| {
                    CoreError::new("not_found", "peer reply Socket is missing", 404)
                })?;
                if socket.owner_route_id() != requester_id
                    || !(socket.acl().message.iter().any(|grant| grant == &peer_id)
                        || socket.acl().message.iter().any(|grant| grant == &actor))
                {
                    return Err(CoreError::forbidden(
                        "requester Socket does not currently authorize this peer to send Messages",
                    ));
                }
                let principal = Principal::new(&actor, &peer_id).map_err(ipc_error)?;
                state
                    .deliver_message(
                        &principal,
                        &task_socket_id,
                        request_id,
                        outbound_message_id,
                        text,
                        Lifecycle::Persistent,
                        Some(task_identity),
                        Some("peer-task-reply".into()),
                    )
                    .map(|outcome| outcome.receipt)
                    .map_err(ipc_error)
            })
            .await?;
        let result = json!({
            "task_id":task_id,"reply_id":reply_id,"attempt_id":attempt_id,"source_id":source_id,
            "peer_chain_id":peer_chain_id,"requester_chain_id":requester_chain_id,
            "state":"reply_delivered","message_delivered":true,"requester_adopted_cb":false,
            "replayed":false,"receipt":receipt
        });
        sqlx::query("INSERT INTO sic_peer_task_replies(task_id,attempt_id,reply_id,fingerprint,result) VALUES(?,?,?,?,?)")
            .bind(task_id).bind(attempt_id).bind(reply_id).bind(fingerprint)
            .bind(storage::json_text(&result)?)
            .execute(&mut **tx).await?;
        Ok(result)
    }

    /// Durable peer task lifecycle is host-only. A successful claim is a
    /// single-use permission to dispatch externally; replays never regrant it.
    async fn dispatch_peer_task(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &Agent,
        context: &OperationContext,
        operation: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        let fields = obj(p)?;
        let actor = &context.actor;
        let chain = &agent.chain_id;
        match operation {
            "peer.tasks" => {
                if fields
                    .keys()
                    .any(|key| !["offset", "limit"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown peer task query field"));
                }
                let offset = p
                    .get("offset")
                    .map(|_| field_u64(p, "offset"))
                    .transpose()?
                    .unwrap_or(0);
                let limit = p
                    .get("limit")
                    .map(|_| field_u64(p, "limit"))
                    .transpose()?
                    .unwrap_or(100);
                let tasks = crate::peer_tasks::list_for_chain_in_transaction(
                    tx, actor, chain, offset, limit,
                )
                .await?;
                Ok(json!({"chain_id":chain,"tasks":tasks,"offset":offset,"limit":limit}))
            }
            "peer.task.claim" => {
                if fields
                    .keys()
                    .any(|key| !["task_id", "claim_id"].contains(&key.as_str()))
                {
                    return Err(CoreError::invalid("unknown peer task claim field"));
                }
                let result = crate::peer_tasks::claim_in_transaction(
                    tx,
                    actor,
                    chain,
                    required_str(p, "task_id")?,
                    required_str(p, "claim_id")?,
                )
                .await?;
                *replayed = result["replayed"].as_bool().unwrap_or(false);
                Ok(result)
            }
            "peer.task.reply" => {
                self.reply_peer_task_in_transaction(tx, context, chain, p, replayed)
                    .await
            }
            "peer.task.advance" => {
                if fields.keys().any(|key| {
                    ![
                        "task_id",
                        "claim_id",
                        "event_id",
                        "expected_revision",
                        "state",
                    ]
                    .contains(&key.as_str())
                }) {
                    return Err(CoreError::invalid("unknown peer task advance field"));
                }
                crate::peer_tasks::advance_in_transaction(
                    tx,
                    actor,
                    chain,
                    required_str(p, "task_id")?,
                    required_str(p, "claim_id")?,
                    required_str(p, "event_id")?,
                    field_u64(p, "expected_revision")?,
                    required_str(p, "state")?,
                )
                .await
            }
            "peer.task.unknown" => {
                if fields.keys().any(|key| {
                    ![
                        "task_id",
                        "claim_id",
                        "event_id",
                        "expected_revision",
                        "reason",
                    ]
                    .contains(&key.as_str())
                }) {
                    return Err(CoreError::invalid(
                        "unknown peer task unknown-outcome field",
                    ));
                }
                let reason = match p.get("reason") {
                    None => "",
                    Some(Value::String(value)) => value.as_str(),
                    Some(_) => {
                        return Err(CoreError::invalid("peer task unknown reason must be text"));
                    }
                };
                crate::peer_tasks::isolate_unknown_in_transaction(
                    tx,
                    actor,
                    chain,
                    required_str(p, "task_id")?,
                    required_str(p, "claim_id")?,
                    required_str(p, "event_id")?,
                    field_u64(p, "expected_revision")?,
                    reason,
                )
                .await
            }
            _ => Err(CoreError::new(
                "unsupported_operation",
                "peer task operation is not implemented",
                501,
            )),
        }
    }

    /// These actions are exposed only to the authenticated host control plane;
    /// they are never callable by the model's fixed tool dispatcher.
    async fn dispatch_host_tool(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &mut Agent,
        context: &OperationContext,
        operation: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        let chain = &agent.chain_id;
        let actor = &context.actor;
        ToolStore::initialize_in_transaction(tx).await?;
        match operation {
            "tools.configure" | "tools.sync" => {
                let acl: ToolAcl = serde_json::from_value(
                    p.get("acl")
                        .cloned()
                        .ok_or_else(|| CoreError::invalid("host tool ACL required"))?,
                )?;
                let rows = p
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| CoreError::invalid("host tools array required"))?;
                let mut tools = Vec::new();
                for row in rows {
                    let fields = obj(row)?;
                    if fields
                        .keys()
                        .any(|key| key != "descriptor" && key != "host_fixed")
                    {
                        return Err(CoreError::invalid("unknown host tool field"));
                    }
                    let descriptor: ToolDescriptor = serde_json::from_value(
                        row.get("descriptor")
                            .cloned()
                            .ok_or_else(|| CoreError::invalid("tool descriptor required"))?,
                    )?;
                    let fixed = row
                        .get("host_fixed")
                        .map(|v| {
                            v.as_bool()
                                .ok_or_else(|| CoreError::invalid("host_fixed must be boolean"))
                        })
                        .transpose()?
                        .unwrap_or(false);
                    tools.push((descriptor, fixed));
                }
                let catalog_digest = storage::digest(p)?;
                let existing: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM sic_dynamic_toolsets WHERE chain_id=? AND actor=?",
                )
                .bind(chain)
                .bind(actor)
                .fetch_optional(&mut **tx)
                .await?;
                if operation == "tools.configure" || existing.is_none() {
                    ToolStore::configure_from_host_in_transaction(tx, chain, actor, acl, tools)
                        .await?;
                } else {
                    ToolStore::sync_from_host_in_transaction(tx, chain, actor, acl, tools).await?;
                }
                let revision = storage::uid("tool_catalog_revision");
                storage::put_object(tx,"tool_catalog_revision",&revision,&json!({"chain_id":chain,"actor":actor,
                    "operation":operation,"catalog_digest":catalog_digest,"catalog":p,"time":now()})).await?;
                Ok(
                    json!({"configured":true,"catalog_revision":revision,"catalog_digest":catalog_digest}),
                )
            }
            "tools.update_acl" => {
                let acl: ToolAcl = serde_json::from_value(
                    p.get("acl")
                        .cloned()
                        .ok_or_else(|| CoreError::invalid("host tool ACL required"))?,
                )?;
                ToolStore::update_acl_from_host_in_transaction(tx, chain, actor, acl).await?;
                Ok(json!({"updated":true}))
            }
            "tools.register" => {
                let descriptor: ToolDescriptor = serde_json::from_value(
                    p.get("descriptor")
                        .cloned()
                        .ok_or_else(|| CoreError::invalid("tool descriptor required"))?,
                )?;
                let fixed = p
                    .get("host_fixed")
                    .map(|v| {
                        v.as_bool()
                            .ok_or_else(|| CoreError::invalid("host_fixed must be boolean"))
                    })
                    .transpose()?
                    .unwrap_or(false);
                ToolStore::register_tool_from_host_in_transaction(
                    tx, chain, actor, descriptor, fixed,
                )
                .await?;
                Ok(json!({"registered":true}))
            }
            "tools.status" => {
                let catalog =
                    ToolStore::search_descriptions_in_transaction(tx, chain, actor, "").await?;
                let leases = ToolStore::active_leases_in_transaction(tx, chain, actor).await?;
                Ok(json!({"catalog":catalog,"active_leases":leases}))
            }
            "tools.tasks" => {
                let revoked = match check_dependencies(tx, agent).await {
                    Ok(()) => false,
                    Err(error) if error.code == "forbidden" => true,
                    Err(error) => return Err(error),
                };
                let rows = sqlx::query("SELECT state FROM host_tool_tasks ORDER BY id")
                    .fetch_all(&mut **tx)
                    .await?;
                let mut tasks = Vec::new();
                for row in rows {
                    let encoded: String = row.try_get("state")?;
                    let task: Value = serde_json::from_str(&encoded)?;
                    if task["chain_id"] == *chain && task["actor"] == *actor {
                        if revoked {
                            tasks.push(json!({"task_id":task["task_id"],"status":task["status"],"revoked":true}));
                        } else {
                            tasks.push(task);
                        }
                    }
                }
                Ok(json!({"tasks":tasks}))
            }
            "tool.claim" => {
                let fields = obj(p)?;
                if fields
                    .keys()
                    .any(|key| key != "task_id" && key != "claim_id")
                {
                    return Err(CoreError::invalid("unknown tool claim field"));
                }
                let id = required_str(p, "task_id")?;
                let claim_id = required_str(p, "claim_id")?;
                if claim_id.trim().is_empty() || claim_id.len() > 128 {
                    return Err(CoreError::invalid(
                        "claim_id must be nonempty and at most 128 bytes",
                    ));
                }
                let row = sqlx::query("SELECT state FROM host_tool_tasks WHERE id=?")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(|| CoreError::new("not_found", "host tool task not found", 404))?;
                let encoded: String = row.try_get("state")?;
                let mut state: Value = serde_json::from_str(&encoded)?;
                if state["chain_id"] != *chain || state["actor"] != *actor {
                    return Err(CoreError::forbidden("host tool task is outside this chain"));
                }
                match state["status"].as_str() {
                    Some(
                        "claimed"
                        | "completed"
                        | "delivered_unattested"
                        | "reconciled_revoked"
                        | "unknown",
                    ) if state["claim_id"] == claim_id && state["claimed_by"] == *actor => {
                        *replayed = true;
                        state["execution_permit"] = json!(false);
                        return Ok(state);
                    }
                    Some("pending") => {}
                    Some("claimed") => {
                        return Err(CoreError::conflict("host tool task is already claimed"));
                    }
                    _ => {
                        return Err(CoreError::conflict(
                            "host tool task is no longer available for execution",
                        ));
                    }
                }
                state["status"] = json!("claimed");
                state["claim_id"] = json!(claim_id);
                state["claimed_by"] = json!(actor);
                state["claimed_at"] = json!(now());
                let persisted = storage::json_text(&state)?;
                let changed =
                    sqlx::query("UPDATE host_tool_tasks SET state=? WHERE id=? AND state=?")
                        .bind(persisted)
                        .bind(id)
                        .bind(encoded)
                        .execute(&mut **tx)
                        .await?;
                if changed.rows_affected() != 1 {
                    return Err(CoreError::conflict("host tool task claim raced"));
                }
                state["execution_permit"] = json!(true);
                Ok(state)
            }
            "tool.complete" => {
                let fields = obj(p)?;
                if fields.keys().any(|key| {
                    !["task_id", "claim_id", "succeeded", "result", "delivery"]
                        .contains(&key.as_str())
                }) {
                    return Err(CoreError::invalid("unknown tool completion field"));
                }
                let id = required_str(p, "task_id")?;
                let row = sqlx::query("SELECT state FROM host_tool_tasks WHERE id=?")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(|| CoreError::new("not_found", "host tool task not found", 404))?;
                let encoded: String = row.try_get("state")?;
                let mut state: Value = serde_json::from_str(&encoded)?;
                if state["chain_id"] != *chain || state["actor"] != *actor {
                    return Err(CoreError::forbidden("host tool task is outside this chain"));
                }
                let succeeded = match p.get("succeeded") {
                    Some(Value::Bool(value)) => Some(*value),
                    Some(Value::Null) => None,
                    _ => {
                        return Err(CoreError::invalid(
                            "succeeded must be boolean or null when outcome is unknown",
                        ));
                    }
                };
                let result = p
                    .get("result")
                    .ok_or_else(|| CoreError::invalid("host result required"))?;
                // A native harness can deliver a tool's output without being
                // able to attest whether the external action succeeded. This
                // is different from having no result after an uncertain run.
                let delivery = match p.get("delivery") {
                    None => None,
                    Some(Value::String(value)) if value == "received" => Some("received"),
                    Some(_) => {
                        return Err(CoreError::invalid("delivery must be received when present"));
                    }
                };
                if delivery.is_some() && (succeeded.is_some() || !result.is_string()) {
                    return Err(CoreError::invalid(
                        "received delivery requires text output and unknown success",
                    ));
                }
                let fingerprint = if delivery.is_some() {
                    storage::digest(
                        &json!({"succeeded":succeeded,"result":result,"delivery":delivery}),
                    )?
                } else {
                    // Preserve fingerprints of receipts created before the
                    // delivery distinction was introduced.
                    storage::digest(&json!({"succeeded":succeeded,"result":result}))?
                };
                let supplied_claim = match p.get("claim_id") {
                    None => None,
                    Some(Value::String(value))
                        if !value.trim().is_empty() && value.len() <= 128 =>
                    {
                        Some(value.as_str())
                    }
                    Some(_) => {
                        return Err(CoreError::invalid(
                            "claim_id must be nonempty text of at most 128 bytes",
                        ));
                    }
                };
                if delivery.is_some() && supplied_claim.is_none() {
                    return Err(CoreError::conflict(
                        "received tool output requires the active host claim",
                    ));
                }
                if state["status"] == "completed"
                    || state["status"] == "delivered_unattested"
                    || state["status"] == "reconciled_revoked"
                    || state["status"] == "unknown"
                {
                    if state["result_fingerprint"] != fingerprint {
                        return Err(CoreError::conflict("completed tool task cannot be changed"));
                    }
                    let expected_claim = state["claim_id"].as_str();
                    if expected_claim != supplied_claim {
                        return Err(CoreError::conflict("tool completion claim does not match"));
                    }
                    *replayed = true;
                    return Ok(state["completion"].clone());
                }
                match state["status"].as_str() {
                    Some("pending") if succeeded.is_none() && supplied_claim.is_none() => {}
                    Some("pending") => {
                        return Err(CoreError::conflict(
                            "tool task must be claimed before reporting execution",
                        ));
                    }
                    Some("claimed")
                        if supplied_claim == state["claim_id"].as_str()
                            && state["claimed_by"] == *actor => {}
                    Some("claimed") => {
                        return Err(CoreError::conflict(
                            "tool completion claim does not match the active host claim",
                        ));
                    }
                    _ => return Err(CoreError::conflict("tool task is not completable")),
                }
                let lease: ExecutionLease = serde_json::from_value(state["lease"].clone())?;
                let scope = AuthenticatedWorkerScope::from_authenticated_host(actor, chain, true)?;
                let persisted = tool_dispatch::persist_tool_result_in_transaction(
                    tx, &scope, &lease, succeeded, result,
                )
                .await?;
                let revoked = match check_dependencies(tx, agent).await {
                    Ok(()) => false,
                    Err(error) if error.code == "forbidden" => true,
                    Err(error) => return Err(error),
                };
                let completion = if revoked {
                    json!({"task_id":id,"status":"reconciled_revoked","reconciled_only":true,"lease_id":persisted.lease_id})
                } else {
                    let content = storage::json_text(&json!({"ok":succeeded,"result":result}))?;
                    let mut ignored = false;
                    let added=self.ingest(tx,agent,"main",&json!({"request":{"messages":[{"role":"tool","tool_call_id":state["call_id"],"content":content}]}}),&mut ignored).await?;
                    if added["source_id"] != state["source_id"] {
                        return Err(CoreError::conflict("tool result source group changed"));
                    }
                    let status = if delivery.is_some() {
                        "delivered_unattested"
                    } else if succeeded.is_none() {
                        "unknown"
                    } else {
                        "completed"
                    };
                    let mut completion = json!({"task_id":id,"status":status,"reconciled_only":false,"source_id":added["source_id"],
                        "open_calls":added["open_calls"],"lease_id":persisted.lease_id});
                    if delivery.is_some() {
                        completion["delivery"] = json!("received");
                        completion["succeeded"] = Value::Null;
                    }
                    completion
                };
                state["status"] = json!(if revoked {
                    "reconciled_revoked"
                } else if delivery.is_some() {
                    "delivered_unattested"
                } else if succeeded.is_none() {
                    "unknown"
                } else {
                    "completed"
                });
                state["result_fingerprint"] = json!(fingerprint);
                state["completion"] = completion.clone();
                let changed =
                    sqlx::query("UPDATE host_tool_tasks SET state=? WHERE id=? AND state=?")
                        .bind(storage::json_text(&state)?)
                        .bind(id)
                        .bind(encoded)
                        .execute(&mut **tx)
                        .await?;
                if changed.rows_affected() != 1 {
                    return Err(CoreError::conflict("host tool task completion raced"));
                }
                Ok(completion)
            }
            _ => Err(CoreError::new(
                "unsupported_operation",
                "host tool operation is not implemented",
                501,
            )),
        }
    }
}

pub(crate) fn validate_peer_prepare_policy(
    policy: &crate::scheduler::SchedulerPolicy,
) -> Result<()> {
    if policy.editor_mode != crate::scheduler::EditorMode::Provider {
        return Err(CoreError::invalid(
            "peer.task.prepare requires Provider Editor mode",
        ));
    }
    if !policy.flush {
        return Err(CoreError::invalid(
            "peer.task.prepare requires explicit flush to avoid leaving the spawn task queued",
        ));
    }
    if policy.max_actions == 0 || policy.max_actions > 64 {
        return Err(CoreError::invalid("prepare max_actions must be 1..64"));
    }
    if policy.max_model_calls == 0 || policy.max_model_calls > 8 {
        return Err(CoreError::invalid("prepare max_model_calls must be 1..8"));
    }
    if policy.max_level == 0 || policy.max_level > 3 {
        return Err(CoreError::invalid("prepare max_level must be 1..3"));
    }
    if policy.context_id.trim().is_empty()
        || policy.min_sources_per_wb == 0
        || policy.max_sources_per_wb < policy.min_sources_per_wb
        || policy.max_sources_per_wb > 8
        || policy.max_units_per_wb == 0
        || policy.max_units_per_wb > 512
    {
        return Err(CoreError::invalid(
            "prepare context and WB source/unit bounds are invalid",
        ));
    }
    if !policy.program_rules.is_empty() {
        return Err(CoreError::invalid(
            "program fixture rules are not accepted in Provider prepare mode",
        ));
    }
    let unexpected_level = policy
        .level_batch_sizes
        .keys()
        .any(|level| *level < 2 || *level > policy.max_level);
    if unexpected_level {
        return Err(CoreError::invalid(
            "prepare level batch sizes must match levels 2..max_level",
        ));
    }
    if (2..=policy.max_level).any(|level| {
        policy
            .level_batch_sizes
            .get(&level)
            .is_none_or(|size| *size == 0)
    }) {
        return Err(CoreError::invalid(
            "prepare requires a positive CB batch size for every hierarchy level",
        ));
    }
    Ok(())
}

fn peer_prepare_replay(
    admission: &crate::peer_tasks::PrepareAdmission,
    task_id: &str,
    prepare_id: &str,
) -> Value {
    let mut result = admission.historical_result.clone().unwrap_or_else(|| {
        json!({
            "task_id":task_id,
            "prepare_id":prepare_id,
            "peer_chain_id":admission.task["peer_chain_id"],
            "state":admission.prepare_state,
            "ready_for_dsh":false,
            "evidence":{"provider_called":false,"worker_run":false,
                        "external_tools_executed":false,"physical_cache_hit":"unknown"}
        })
    });
    result["replayed"] = json!(true);
    result["execution_started"] = json!(false);
    result["this_call_provider_called"] = json!(false);
    json!({
        "ok":true,
        "operation":"peer.task.prepare",
        "context":{"peer_chain_id":admission.task["peer_chain_id"]},
        "evidence":{"provider_called":false,"replayed":true,"physical_cache_hit":"unknown"},
        "result":result
    })
}

fn parse_board_acl(value: Option<&Value>, owner: &str) -> Result<BoardAcl> {
    let default =
        json!({"readers":[owner],"writers":[owner],"claimers":[owner],"managers":[owner]});
    let value = value.unwrap_or(&default);
    let fields = obj(value)?;
    if fields
        .keys()
        .any(|key| !["readers", "writers", "claimers", "managers"].contains(&key.as_str()))
    {
        return Err(CoreError::invalid("unknown board ACL field"));
    }
    let list = |key: &str| -> Result<Vec<String>> {
        value
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| CoreError::invalid("board ACL lists required"))?
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| CoreError::invalid("board ACL identity must be nonempty"))
            })
            .collect()
    };
    BoardAcl::new(
        list("readers")?,
        list("writers")?,
        list("claimers")?,
        list("managers")?,
    )
}

impl Core {
    /// Explicit host reconciliation after a crash or uncertain request. No
    /// unknown provider call is retried and the old claim is invalidated.
    async fn abandon_inflight(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        p: &Value,
    ) -> Result<Value> {
        let kind = required_str(p, "kind")?;
        let claim = match kind {
            "worker" => agent.worker_claim.take(),
            "editor" => {
                let id = required_str(p, "job_id")?;
                let job = agent
                    .jobs
                    .get_mut(id)
                    .ok_or_else(|| CoreError::invalid("job not found"))?;
                if job.status == "inflight" {
                    job.status = "failed".into();
                    job.claim_id.take()
                } else {
                    None
                }
            }
            _ => return Err(CoreError::invalid("abandon kind must be worker or editor")),
        };
        if let Some(claim_id) = &claim {
            let record = storage::uid("runtime_abandon");
            storage::put_object(
                conn,
                "runtime_abandon",
                &record,
                &json!({"kind":kind,"claim_id":claim_id,
                "job_id":p.get("job_id"),"outcome":"unknown","time":now()}),
            )
            .await?;
            agent
                .events
                .push(json!({"type":"runtime_abandoned","record_id":record,"time":now()}));
        }
        Ok(
            json!({"abandoned":claim.is_some(),"abandoned_claim_id":claim,"provider_outcome":"unknown"}),
        )
    }
}

fn ipc_error(message: String) -> CoreError {
    CoreError::new("ipc_rejected", message, 403)
}
fn lifecycle(value: &Value) -> Result<Lifecycle> {
    match value.as_str().unwrap_or("V") {
        "P" => Ok(Lifecycle::Persistent),
        "V" => Ok(Lifecycle::Volatile),
        _ => Err(CoreError::invalid("message lifecycle must be P or V")),
    }
}

impl Core {
    fn ipc_store(&self) -> Result<IpcStore> {
        IpcStore::new(self.store.clone(), "sic-agent-workspace")
    }

    async fn ensure_ipc_route(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &Agent,
        actor: &str,
    ) -> Result<()> {
        let principal = Principal::new(actor, &agent.chain_id).map_err(ipc_error)?;
        self.ipc_store()?
            .transact_in_transaction(tx, |state| {
                if state.route(&agent.chain_id).is_none() {
                    state
                        .create_route(&agent.chain_id, actor, agent.creators.clone(), &principal)
                        .map_err(ipc_error)?;
                }
                Ok(())
            })
            .await
    }

    /// Take a historic snapshot only from an actual ancestor cut. References
    /// remain shared immutable objects and retain revocation checks.
    async fn checkpoint_snapshot(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        wanted: &str,
    ) -> Result<(Agent, String)> {
        let mut cursor = agent.current_cut.clone();
        let cut = loop {
            let id =
                cursor.ok_or_else(|| CoreError::forbidden("checkpoint cut is not an ancestor"))?;
            let value = storage::get_object(conn, &id, Some("cut")).await?;
            if id == wanted {
                break value;
            }
            cursor = value["previous_cut"].as_str().map(str::to_owned);
        };
        let context = required_str(&cut, "published_context")?;
        let level = cut["published_level"]
            .as_u64()
            .filter(|value| *value > 0 && *value <= 128)
            .ok_or_else(|| CoreError::invalid("checkpoint published level missing"))?
            as u32;
        let cb_id = cut["contexts"][context]["levels"][level.to_string()]["cbs"]
            .as_array()
            .and_then(|rows| rows.last())
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("checkpoint must name a CB publication cut"))?
            .to_owned();
        storage::get_object(conn, &cb_id, Some("cb")).await?;
        let mut snapshot = agent.clone();
        snapshot.contexts = serde_json::from_value(cut["contexts"].clone())?;
        snapshot.groups = serde_json::from_value(cut["groups"].clone())?;
        snapshot.role_views = serde_json::from_value(cut["role_views"].clone())?;
        snapshot.protected = serde_json::from_value(cut["protected"].clone())?;
        snapshot.board_id = cut["board_id"].as_str().map(str::to_owned);
        snapshot.config = cut["config"].clone();
        snapshot.decision = cut
            .get("decision")
            .cloned()
            .or_else(|| {
                cut["contexts"]["main"]["levels"]["1"]["instant"]
                    .get("decision_tree")
                    .cloned()
            })
            .unwrap_or_else(decision::empty);
        snapshot.input_epoch = cut["input_epoch"].as_u64().unwrap_or(0);
        snapshot.control_epoch = cut["control_epoch"].as_u64().unwrap_or(0);
        snapshot.jobs.clear();
        snapshot.events.clear();
        snapshot.worker_claim = None;
        snapshot.cut_seq = field_u64(&cut, "sequence")?;
        snapshot.current_cut = Some(wanted.into());
        Ok((snapshot, cb_id))
    }

    async fn dispatch_ipc(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &mut Agent,
        context: &OperationContext,
        operation: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        self.ensure_ipc_route(tx, agent, &context.actor).await?;
        let principal = Principal::new(&context.actor, &agent.chain_id).map_err(ipc_error)?;
        let ipc = self.ipc_store()?;
        match operation {
            "chain.checkpoint"=>{
                let cut=p.get("cut_id").and_then(Value::as_str).map(str::to_owned).or_else(||agent.current_cut.clone())
                    .ok_or_else(||CoreError::conflict("a published cut is required"))?;
                let (snapshot,cb_id)=self.checkpoint_snapshot(tx,agent,&cut).await?;
                let checkpoint=storage::uid("checkpoint");
                let value=as_value(&snapshot)?;
                ipc.transact_in_transaction(tx,|state|state.record_checkpoint(&principal,&checkpoint,&cut,&cb_id,snapshot.cut_seq,value).map_err(ipc_error)).await?;
                storage::put_object(tx,"checkpoint",&checkpoint,&json!({"checkpoint_id":checkpoint,"chain_id":agent.chain_id,"cut_id":cut,"cb_id":cb_id})).await?;
                Ok(json!({"checkpoint_id":checkpoint,"cut_id":cut,"cb_id":cb_id,"sequence":snapshot.cut_seq}))
            },
            "socket.create"=>{
                let checkpoint=required_str(p,"checkpoint_id")?;
                let acl:SocketAcl=serde_json::from_value(p.get("acl").cloned().unwrap_or_else(||json!({"message":[],"signal":[],"spawn":[],"read":[]})))?;
                let socket=storage::uid("socket");
                ipc.transact_in_transaction(tx,|state|state.create_socket(&principal,&socket,checkpoint,acl).map_err(ipc_error)).await?;
                Ok(json!({"socket_id":socket,"checkpoint_id":checkpoint}))
            },
            "socket.acl.update"=>{
                let socket=required_str(p,"socket_id")?;
                let acl:SocketAcl=serde_json::from_value(p.get("acl").cloned().ok_or_else(||CoreError::invalid("Socket ACL required"))?)?;
                let previous=ipc.transact_in_transaction(tx,|state|state.update_socket_acl(&principal,socket,acl.clone()).map_err(ipc_error)).await?;
                let revision=storage::uid("socket_acl_revision");
                storage::put_object(tx,"socket_acl_revision",&revision,&json!({"socket_id":socket,"chain_id":agent.chain_id,
                    "previous_acl":previous,"acl":acl,"actor":context.actor,"time":now()})).await?;
                Ok(json!({"socket_id":socket,"acl_revision":revision}))
            },
            "socket.read"=>{
                let socket=required_str(p,"socket_id")?;
                let (snapshot,checkpoint)=ipc.transact_in_transaction(tx,|state| {
                    let snapshot=state.read_checkpoint(&principal,socket).map_err(ipc_error)?;
                    let checkpoint=state.socket(socket).ok_or_else(||CoreError::invalid("Socket missing"))?.checkpoint_id().to_owned();
                    Ok((snapshot,checkpoint))
                }).await?;
                let checked=as_agent(snapshot)?;check_dependencies(tx,&checked).await?;
                let ids=self.selected_ids(tx,&checked,"worker").await?;
                let offset=match p.get("offset") {Some(_) => usize::try_from(field_u64(p,"offset")?)
                    .map_err(|_|CoreError::invalid("Socket page offset too large"))?,None => 0};
                let limit=match p.get("limit") {Some(_) => usize::try_from(field_u64(p,"limit")?)
                    .map_err(|_|CoreError::invalid("Socket page limit too large"))?,None => 2048};
                if limit==0 || limit>8192 {return Err(CoreError::invalid("Socket page limit must be 1..8192"));}
                let mut text=String::new();
                for (index,id) in ids.iter().enumerate() {
                    let cb=storage::get_object(tx,id,Some("cb")).await?;
                    if index>0 {text.push('\n');}
                    text.push_str(cb["body"].as_str().ok_or_else(||CoreError::invalid("CB body missing"))?);
                }
                let total=text.chars().count();let page:String=text.chars().skip(offset).take(limit).collect();
                let next=offset.saturating_add(page.chars().count());
                Ok(json!({"socket_id":socket,"checkpoint_id":checkpoint,"cut_id":checked.current_cut,"cb_ids":ids,
                    "value":page,"offset":offset,"total":total,"next_offset":if next<total {Some(next)}else{None},"unit":"unicode-codepoints"}))
            },
            "socket.inbox"=>{
                ipc.transact_in_transaction(tx,|state| {
                    let route=state.route(&agent.chain_id).ok_or_else(||CoreError::invalid("IPC route missing"))?;
                    Ok(json!({"chain_id":agent.chain_id,"status":route.status(),"messages":route.inbox()}))
                }).await
            },
            "socket.message"=>{
                let socket=required_str(p,"socket_id")?;let request=required_str(p,"request_id")?;
                let message=required_str(p,"message_id")?;let text=required_str(p,"text")?;
                let life=lifecycle(&p["lifecycle"])?;
                let task=p.get("task_id").and_then(Value::as_str).map(str::to_owned);
                let phase=p.get("phase_id").and_then(Value::as_str).map(str::to_owned);
                let outcome=ipc.transact_in_transaction(tx,|state|state.deliver_message(&principal,socket,request,message,text,life,task,phase).map_err(ipc_error)).await?;
                *replayed=outcome.replayed;Ok(json!({"receipt":outcome.receipt}))
            },
            "socket.signal"=>{
                let socket=required_str(p,"socket_id")?;let id=required_str(p,"signal_id")?;
                let action:SignalAction=serde_json::from_value(p.get("action").cloned().ok_or_else(||CoreError::invalid("signal action required"))?)?;
                let (outcome,owner)=ipc.transact_in_transaction(tx,|state| {
                    let outcome=state.signal(&principal,socket,id,action).map_err(ipc_error)?;
                    let owner=state.route(&outcome.receipt.target_route_id).ok_or_else(||CoreError::invalid("target route missing"))?.owner_actor_id().to_owned();
                    Ok((outcome,owner))
                }).await?;
                let target=outcome.receipt.target_route_id.clone();
                let state_name=match outcome.receipt.resulting_status {RouteStatus::Running=>"running",RouteStatus::Waiting=>"waiting",RouteStatus::Stopped=>"stopped"};
                if !outcome.replayed && outcome.receipt.transitioned {
                    if target==agent.chain_id {agent.run_state=state_name.into();agent.control_epoch=agent.control_epoch.saturating_add(1);}
                    else {let mut peer=load_agent(tx,&target,&owner).await?;peer.run_state=state_name.into();
                        peer.control_epoch=peer.control_epoch.saturating_add(1);save_agent(tx,&peer).await?;}
                }
                *replayed=outcome.replayed;Ok(json!({"receipt":outcome.receipt}))
            },
            "socket.spawn"=>{
                let socket=required_str(p,"socket_id")?;let request=required_str(p,"request_id")?;
                let message=required_str(p,"message_id")?;let text=required_str(p,"text")?;
                let life=lifecycle(&p["lifecycle"])?;
                let lifecycle_code = match &life {
                    Lifecycle::Persistent => "P",
                    Lifecycle::Volatile => "V",
                };
                let key=format!("{}:spawn:{request}",agent.chain_id);
                let fingerprint=storage::digest(p)?;
                if let Some(row)=sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?").bind(&key).fetch_optional(&mut **tx).await? {
                    let old:String=row.try_get("fingerprint")?;if old!=fingerprint {return Err(CoreError::conflict("spawn request id reused with different content"));}
                    let result:String=row.try_get("result")?;*replayed=true;return Ok(serde_json::from_str(&result)?);
                }
                let new_id=storage::uid("chain");
                let (snapshot,checkpoint)=ipc.transact_in_transaction(tx,|state| {
                    state.spawn_peer(&principal,socket,&new_id,request,message,text,life.clone()).map_err(ipc_error)?;
                    let route=state.route(&new_id).ok_or_else(||CoreError::invalid("spawned route missing"))?;
                    Ok((route.initial_snapshot().clone(),route.origin_checkpoint().unwrap_or_default().to_owned()))
                }).await?;
                let mut peer=as_agent(snapshot)?;check_dependencies(tx,&peer).await?;
                peer.chain_id=new_id.clone();peer.owners=vec![context.actor.clone()];peer.origin_checkpoint=Some(checkpoint.clone());
                peer.jobs.clear();peer.events.clear();peer.worker_claim=None;peer.run_state="running".into();
                // A quiet fork needs an empty WB boundary so its inbox can be
                // atomically incorporated on the next publication.
                if peer.contexts.values().all(|c|c.levels.values().all(|l|l.pending.is_empty())) {
                    let mut ignored=false;
                    self.ingest(tx,&mut peer,"main",&json!({"request":{"messages":[{"role":"user","content":""}]}}),&mut ignored).await?;
                }
                save_agent(tx,&peer).await?;
                let task_id=storage::uid("peer_task");
                let task=crate::peer_tasks::record_spawn_in_transaction(
                    tx,&task_id,&context.actor,&agent.chain_id,&new_id,socket,&checkpoint,
                    request,message,lifecycle_code,
                ).await?;
                let result=json!({"chain_id":new_id,"origin_checkpoint":checkpoint,"relationship":"peer","task_queued":true,"task_id":task["task_id"]});
                sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)").bind(&key).bind(fingerprint)
                    .bind(storage::json_text(&result)?).execute(&mut **tx).await?;
                Ok(result)
            },
            _=>Err(CoreError::new("unsupported_operation","IPC operation is not implemented",501))
        }
    }

    async fn publish_with_ipc(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        agent: &mut Agent,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        let actor = agent
            .owners
            .first()
            .ok_or_else(|| CoreError::invalid("Agent owner missing"))?
            .clone();
        self.ensure_ipc_route(tx, agent, &actor).await?;
        let job = agent
            .jobs
            .get(required_str(p, "job_id")?)
            .cloned()
            .ok_or_else(|| CoreError::invalid("job not found"))?;
        let worker = agent
            .role_views
            .get("worker")
            .ok_or_else(|| CoreError::invalid("Worker view missing"))?;
        let eligible = job.context_id == worker.context_id && job.level <= worker.level;
        let board_i = if let Some(id) = &agent.board_id {
            let principal = BoardActor::from_authenticated_host(&agent.chain_id)?;
            Some(BoardService::read_latest_i_in_transaction(tx, &principal, id).await?)
        } else {
            None
        };
        let cut = storage::uid("cut");
        let ipc = self.ipc_store()?;
        let preview = ipc
            .transact_in_transaction(tx, |state| {
                state
                    .preview_publication(&agent.chain_id, &cut, eligible)
                    .map_err(ipc_error)
            })
            .await?;
        let result = self
            .publish(
                tx,
                agent,
                p,
                replayed,
                preview.messages(),
                &cut,
                board_i.as_ref(),
            )
            .await?;
        if !*replayed && eligible {
            let cb = required_str(&result, "cb_id")?;
            if !self
                .selected_ids(tx, agent, "worker")
                .await?
                .iter()
                .any(|id| id == cb)
                && !preview.messages().is_empty()
            {
                return Err(CoreError::conflict(
                    "IPC messages cannot be consumed in a cut outside the Worker view",
                ));
            }
            ipc.transact_in_transaction(tx, |state| {
                state
                    .confirm_publication(&preview, cb)
                    .map(|_| ())
                    .map_err(ipc_error)
            })
            .await?;
        }
        Ok(result)
    }
}

impl Core {
    /// A Worker waits for every scheduled Editor and sees uncompressed WB.
    async fn run_worker(&self, context: &OperationContext, p: &Value) -> Result<Value> {
        if p.as_object().is_none_or(|fields| !fields.is_empty()) {
            return Err(CoreError::invalid("worker.run takes an empty payload"));
        }
        let chain_id = context
            .chain_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CoreError::invalid("chain_id required"))?;
        let config = self.model_config.as_ref().ok_or_else(|| {
            CoreError::new(
                "model_unconfigured",
                "no host-configured Worker profile",
                409,
            )
        })?;
        let profile = model_policy::resolve_worker_profile(config).map_err(err)?;
        provider::validate_openai_profile(&profile).map_err(err)?;
        let claim = storage::uid("worker_claim");
        let (
            request,
            prefix_digest,
            base_seq,
            tool_configured,
            control_epoch,
            input_epoch,
            worker_run_id,
        ) = {
            let mut tx = self.store.begin().await?;
            let mut agent = load_agent(&mut tx, chain_id, &context.actor).await?;
            check_dependencies(&mut tx, &agent).await?;
            self.check_board_access(&mut tx, &agent).await?;
            if agent.run_state != "running" || agent.worker_claim.is_some() {
                return Err(CoreError::conflict("Worker is not available"));
            }
            if agent.jobs.values().any(|job| {
                job.base_cut == agent.cut_seq
                    && matches!(
                        job.status.as_str(),
                        "open" | "ready" | "inflight" | "failed"
                    )
            }) {
                return Err(CoreError::new(
                    "editor_pending",
                    "Worker waits for Editor publication, including a failed job that needs explicit replacement",
                    409,
                ));
            }
            let view = agent
                .role_views
                .get("worker")
                .cloned()
                .ok_or_else(|| CoreError::invalid("Worker view missing"))?;
            let ids = self.selected_ids(&mut tx, &agent, "worker").await?;
            if ids != self.selected_ids(&mut tx, &agent, "editor").await? {
                return Err(CoreError::conflict(
                    "shared prefix requires the Worker and Editor to select the same CB view",
                ));
            }
            let mut cbs = Vec::new();
            for id in &ids {
                let cb = storage::get_object(&mut tx, id, Some("cb")).await?;
                cbs.push(prefix::RenderedCb {
                    cb_id: id.clone(),
                    messages: vec![json!({"role":"user","content":cb["body"]})],
                });
            }
            let cut_id = agent
                .current_cut
                .clone()
                .unwrap_or_else(|| format!("genesis:{chain_id}"));
            let plan = prefix::PrefixPlan::freeze_with_instructions(
                prefix::PrefixPlanSpec {
                    model: required_str(&profile, "model")?.into(),
                    provider_id: "configured-host-provider".into(),
                    cache_namespace: chain_id.into(),
                    chain_id: chain_id.into(),
                    context_id: view.context_id.clone(),
                    level: view.level,
                    cut_id: cut_id.clone(),
                    cb_chain: cbs,
                    projection_scope: "selected-shared-cb-view".into(),
                    projection_budget: profile["max_input_tokens"].as_u64().unwrap_or(1),
                    schema_version: "1".into(),
                    tool_version: "1".into(),
                },
                agent.protected.clone(),
            )
            .map_err(err)?;
            let tools = plan.common_tools().map_err(err)?;
            // Refuse provider requests if a future fixed registry outgrows the
            // global schema budget, even when no dynamic catalog is configured.
            crate::tool_store::dynamic_schema_capacity()?;
            ToolStore::initialize_in_transaction(&mut tx).await?;
            let configured: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM sic_dynamic_toolsets WHERE chain_id=? AND actor=?",
            )
            .bind(chain_id)
            .bind(&context.actor)
            .fetch_optional(&mut *tx)
            .await?;
            let tool_configured = configured.is_some();
            let directory = if tool_configured {
                ToolStore::expanded_descriptors_in_transaction(&mut tx, chain_id, &context.actor)
                    .await?
            } else {
                vec![]
            };
            let catalog = if tool_configured {
                ToolStore::search_descriptions_in_transaction(&mut tx, chain_id, &context.actor, "")
                    .await?
            } else {
                vec![]
            };
            let discoverable_tools = json!({
                "tools":catalog.iter().take(20).map(|tool| json!({
                    "name":tool.name,
                    "expanded":tool.expanded
                })).collect::<Vec<_>>(),
                "total":catalog.len(),
                "has_more":catalog.len() > 20
            });
            let visible = tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect();
            let mut activated: Vec<String> = [
                "sic_socket_spawn",
                "sic_socket_read",
                "sic_message_send",
                "sic_message_receive",
                "sic_signal_send",
                "sic_signal_wait",
                "sic_decision_read",
                "sic_decision_write",
                "sic_board_read",
                "sic_board_post",
                "sic_board_claim",
                "sic_board_release",
                "sic_board_progress",
                "sic_memory_page",
                "sic_memory_replay",
                "sic_hierarchy_timeline",
                "sic_hierarchy_expand",
            ]
            .iter()
            .map(|name| name.to_string())
            .collect();
            if tool_configured {
                activated.extend(
                    [
                        "sic_tool_search",
                        "sic_tool_expand",
                        "sic_tool_reclaim",
                        "sic_tool_call",
                    ]
                    .iter()
                    .map(|name| name.to_string()),
                );
            }
            let auth = prefix::HostAuthorization::from_host(prefix::HostAuthorizationSpec {
                plan_digest: plan.digest().into(),
                actor: context.actor.clone(),
                chain_id: chain_id.into(),
                context_id: view.context_id.clone(),
                level: view.level,
                cut_id,
                cb_ids: ids,
                scope_id: format!("worker:{}", agent.cut_seq),
                mode: prefix::Mode::Worker,
                acl_epoch: 0,
                visible_tools: visible,
                allowed_tools: activated,
                editor_scope: None,
            })
            .map_err(err)?;
            let views = json!({"current":agent.level(&view.context_id,view.level)?.instant,"decision_tree":agent.decision,
                "task_goal":agent.config["task_goal"],"task_constraints":agent.config["task_constraints"],
                "dynamic_tool_directory":directory,"discoverable_tools":discoverable_tools,
                "availability":if tool_configured {"Use activated Socket, Signal, Message, decision and dynamic tools only under host ACL. Search tool descriptions with sic_tool_search and expand schemas individually."}else{"Use activated Socket, Signal, Message and decision tools only under host ACL. No external tool catalog is configured."}});
            // Uncommitted WB is part of the Worker context. In particular V
            // messages must be visible before their next compression; merely
            // existing pending input does not mean an Editor was scheduled.
            let mut suffix = Vec::new();
            for root in &agent.level(&view.context_id, 1)?.pending {
                let source_state = source_snapshot(&mut tx, &agent, root, None).await?;
                if source_state["open_calls"]
                    .as_array()
                    .is_some_and(|calls| !calls.is_empty())
                {
                    return Err(CoreError::new(
                        "tool_group_open",
                        "Worker waits for outstanding host tool results",
                        409,
                    ));
                }
                let group = agent
                    .groups
                    .get(root)
                    .ok_or_else(|| CoreError::invalid("pending source group missing"))?;
                for source_id in group {
                    let source = storage::get_object(&mut tx, source_id, Some("source")).await?;
                    let messages = source["sidecar"]["messages"]
                        .as_array()
                        .ok_or_else(|| CoreError::invalid("pending source messages missing"))?;
                    for message in messages {
                        if !matches!(message["role"].as_str(), Some("system" | "developer")) {
                            suffix.push(message.clone());
                        }
                    }
                }
            }
            if suffix.is_empty() {
                suffix.push(json!({"role":"user","content":"Continue the current task."}));
            }
            let built = prefix::worker_request(&plan, &auth, &suffix, &views).map_err(err)?;
            let mut request = built.request;
            request
                .as_object_mut()
                .ok_or_else(|| CoreError::invalid("bad built request"))?
                .remove("model");
            apply_profile_reasoning(&profile, &mut request)?;
            let event = storage::uid("worker_run");
            storage::put_object(
                &mut tx,
                "worker_run",
                &event,
                &json!({"claim_id":claim,"plan":plan,"profile":profile,
                "prefix_digest":built.common_prefix_digest,"started":now()}),
            )
            .await?;
            let seq = agent.cut_seq;
            let control_epoch = agent.control_epoch;
            let input_epoch = agent.input_epoch;
            agent.worker_claim = Some(claim.clone());
            agent
                .events
                .push(json!({"type":"worker_run_started","record_id":event,"time":now()}));
            save_agent(&mut tx, &agent).await?;
            tx.commit().await?;
            (
                request,
                built.common_prefix_digest,
                seq,
                tool_configured,
                control_epoch,
                input_epoch,
                event,
            )
        };
        let count = surrogate_input_count(&request)?;
        let captured = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
        let observed = captured.clone();
        let called = provider::chat_completion_observed(
            &profile,
            &request,
            provider::CallRole::Worker,
            &count,
            |name| (self.secret_resolver)(name),
            move |body| {
                if let Ok(mut slot) = observed.lock() {
                    *slot = Some(body.to_vec());
                }
            },
        )
        .await;
        let (response, raw, usage, call_error) = match called {
            Ok(result) => {
                let original = &result.raw_response["choices"][0]["message"];
                let mut normalized = json!({"role":"assistant","content":original.get("content").cloned().unwrap_or(Value::Null)});
                if let Some(calls) = &result.tool_calls {
                    normalized["tool_calls"] = calls.clone();
                }
                (
                    Some(normalized),
                    Some(result.raw_response),
                    result.usage,
                    None,
                )
            }
            Err(error) => (None, observed_body_value(&captured), None, Some(error)),
        };
        let mut tx = self.store.begin().await?;
        let mut agent = load_agent(&mut tx, chain_id, &context.actor).await?;
        if agent.worker_claim.as_deref() != Some(claim.as_str()) {
            return Err(CoreError::conflict(
                "Worker claim changed during provider call",
            ));
        }
        let dependency_error = match check_dependencies(&mut tx, &agent).await {
            Err(error) => Some(error),
            Ok(()) => self.check_board_access(&mut tx, &agent).await.err(),
        };
        let stale = agent.cut_seq != base_seq;
        let interrupted = agent.run_state != "running" || agent.control_epoch != control_epoch;
        let input_changed = agent.input_epoch != input_epoch;
        let aid = storage::uid("worker_attempt");
        agent.worker_claim = None;
        agent
            .events
            .push(json!({"type":"worker_attempt","record_id":aid,"worker_run_id":worker_run_id,"time":now()}));
        let mut source_id = None;
        let mut host_tasks = Vec::new();
        let mut tool_results = Vec::new();
        if dependency_error.is_none() && !stale && !interrupted && !input_changed {
            if let Some(message) = &response {
                let mut replayed = false;
                let added = self
                    .ingest(
                        &mut tx,
                        &mut agent,
                        "main",
                        &json!({"request":{"messages":[message]}}),
                        &mut replayed,
                    )
                    .await?;
                source_id = added["source_id"].as_str().map(str::to_owned);
                if let Some(event) =
                    agent.events.iter_mut().rev().find(|event| {
                        event["type"] == "worker_attempt" && event["record_id"] == aid
                    })
                {
                    if let Some(source) = source_id.as_deref() {
                        event["source_id"] = json!(source);
                    }
                }
                let calls = message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if !calls.is_empty() {
                    let source = source_id
                        .as_deref()
                        .ok_or_else(|| CoreError::invalid("tool source missing"))?;
                    let scope = AuthenticatedWorkerScope::from_authenticated_host(
                        &context.actor,
                        chain_id,
                        context.allow_model,
                    )?;
                    for call in &calls {
                        let id = required_str(call, "id")?;
                        let name = required_str(&call["function"], "name")?;
                        let args: Value =
                            serde_json::from_str(required_str(&call["function"], "arguments")?)?;
                        let dynamic = [
                            "sic_tool_search",
                            "sic_tool_expand",
                            "sic_tool_reclaim",
                            "sic_tool_call",
                        ]
                        .contains(&name);
                        let immediate = if agent.run_state != "running"
                            || agent.control_epoch != control_epoch
                        {
                            Some(
                                json!({"ok":false,"code":"agent_interrupted","message":"Remaining tool calls were not dispatched after Agent state changed."}),
                            )
                        } else if !dynamic {
                            let identity = format!("{aid}:{id}");
                            let before = agent.clone();
                            sqlx::query("SAVEPOINT model_tool_action")
                                .execute(&mut *tx)
                                .await?;
                            let action = self
                                .dispatch_model_builtin(
                                    &mut tx, &mut agent, context, name, &args, &identity,
                                )
                                .await;
                            if action.is_err() {
                                sqlx::query("ROLLBACK TO SAVEPOINT model_tool_action")
                                    .execute(&mut *tx)
                                    .await?;
                                agent = before;
                            }
                            sqlx::query("RELEASE SAVEPOINT model_tool_action")
                                .execute(&mut *tx)
                                .await?;
                            Some(match action {
                                Ok(result) => json!({"ok":true,"result":result}),
                                Err(error) => {
                                    json!({"ok":false,"code":error.code,"message":error.message})
                                }
                            })
                        } else {
                            let outcome = if tool_configured {
                                tool_dispatch::dispatch_model_tool_in_transaction(
                                    &mut tx, &scope, name, &args,
                                )
                                .await
                            } else {
                                Err(CoreError::forbidden("tool is not activated by the host"))
                            };
                            match outcome {
                                Ok(ToolDispatch::Search {
                                    tools,
                                    offset,
                                    total,
                                    next_offset,
                                }) => Some(json!({"ok":true,"tools":tools,"offset":offset,
                                        "total":total,"next_offset":next_offset})),
                                Ok(ToolDispatch::Expanded { expansion }) => {
                                    Some(json!({"ok":true,"expansion":expansion}))
                                }
                                Ok(ToolDispatch::Reclaimed { reclaimed }) => {
                                    Some(json!({"ok":true,"reclaimed":reclaimed}))
                                }
                                Ok(ToolDispatch::HostExecutionRequired {
                                    lease,
                                    descriptor,
                                    arguments,
                                    evicted,
                                }) => {
                                    let task_id = storage::uid("tool_task");
                                    let task = json!({"task_id":task_id,"chain_id":chain_id,"actor":context.actor,"status":"pending",
                                    "source_id":source,"call_id":id,"lease":lease,"descriptor":descriptor,"arguments":arguments,
                                    "evicted":evicted,"schema_validation":"trusted host must validate before execution"});
                                    sqlx::query(
                                        "INSERT INTO host_tool_tasks(id,state) VALUES(?,?)",
                                    )
                                    .bind(&task_id)
                                    .bind(storage::json_text(&task)?)
                                    .execute(&mut *tx)
                                    .await?;
                                    host_tasks.push(task);
                                    None
                                }
                                Err(error) => Some(
                                    json!({"ok":false,"code":error.code,"message":error.message}),
                                ),
                            }
                        };
                        if let Some(result) = immediate {
                            let content = storage::json_text(&result)?;
                            let added = self
                                .ingest(
                                    &mut tx,
                                    &mut agent,
                                    "main",
                                    &json!({"request":{"messages":[
                                {"role":"tool","tool_call_id":id,"content":content}]}}),
                                    &mut replayed,
                                )
                                .await?;
                            if added["source_id"] != source {
                                return Err(CoreError::conflict(
                                    "tool result source group changed",
                                ));
                            }
                            tool_results.push(json!({"tool_call_id":id,"result":result}));
                        }
                    }
                } else if tool_configured {
                    let scope = AuthenticatedWorkerScope::from_authenticated_host(
                        &context.actor,
                        chain_id,
                        context.allow_model,
                    )?;
                    tool_dispatch::complete_worker_response_in_transaction(&mut tx, &scope, &aid)
                        .await?;
                }
            }
        }
        storage::put_object(&mut tx,"worker_attempt",&aid,&json!({"claim_id":claim,"worker_run_id":worker_run_id,
            "request_digest":storage::digest(&request)?,"input_count":count.tokens,"counter_id":count.counter_id,
            "raw_response":raw,"usage":usage,"error":call_error,"source_id":source_id,
            "stale":stale,"interrupted":interrupted,"input_changed":input_changed,
            "dependencies_revoked":dependency_error.is_some(),"cut_seq":agent.cut_seq,
            "result_input_epoch":agent.input_epoch,"result_control_epoch":agent.control_epoch,
            "finished":now()})).await?;
        save_agent(&mut tx, &agent).await?;
        tx.commit().await?;
        if let Some(error) = dependency_error {
            return Err(error);
        }
        if stale {
            return Err(CoreError::conflict(
                "Worker cut changed while provider was running",
            ));
        }
        if interrupted {
            return Err(CoreError::new(
                "worker_interrupted",
                "Worker result was not adopted after a state change",
                409,
            ));
        }
        if input_changed {
            return Err(CoreError::new(
                "worker_input_changed",
                "Worker result was not adopted because input changed",
                409,
            ));
        }
        if let Some(error) = call_error {
            return Err(CoreError::new("worker_failed", error, 502));
        }
        let message = response.ok_or_else(|| {
            CoreError::new("worker_failed", "Worker did not return a message", 502)
        })?;
        Ok(
            json!({"ok":true,"operation":"worker.run","context":{"chain_id":chain_id,"actor":context.actor},
            "result":{"content":message["content"],"tool_calls":message.get("tool_calls"),"source_id":source_id,"attempt_id":aid,
                      "host_tasks":host_tasks,"tool_results":tool_results},
            "evidence":{"provider_called":true,"physical_cache_hit":"unknown","common_prefix_digest":prefix_digest,
                        "token_count":"host-surrogate-not-provider-usage"}}),
        )
    }
}

impl Core {
    /// Run one claimed Editor outside the SQLite transaction. The claim,
    /// frozen profile, plan and every completed attempt are durable. Provider
    /// transport failures are not automatically retried.
    async fn run_editor(&self, context: &OperationContext, p: &Value) -> Result<Value> {
        let chain_id = context
            .chain_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CoreError::invalid("chain_id required"))?;
        let id = required_str(p, "job_id")?.to_owned();
        if p.as_object()
            .is_none_or(|fields| fields.keys().any(|key| key != "job_id"))
        {
            return Err(CoreError::invalid("edit.run accepts only job_id"));
        }
        let claim = storage::uid("editor_claim");
        let (job, profile, mut request, prefix_digest, control_epoch, frozen_decision) = {
            let mut tx = self.store.begin().await?;
            let mut agent = load_agent(&mut tx, chain_id, &context.actor).await?;
            check_dependencies(&mut tx, &agent).await?;
            self.check_board_access(&mut tx, &agent).await?;
            if agent.run_state != "running" {
                return Err(CoreError::conflict("Agent is not running"));
            }
            if agent.worker_claim.is_some() {
                return Err(CoreError::conflict("Worker is still running"));
            }
            if agent.jobs.values().any(|job| job.status == "inflight") {
                return Err(CoreError::conflict("the single Editor is already running"));
            }
            let job = agent
                .jobs
                .get(&id)
                .cloned()
                .ok_or_else(|| CoreError::invalid("job not found"))?;
            if job.status != "open" || job.base_cut != agent.cut_seq {
                return Err(CoreError::conflict(
                    "Editor job is not open at its frozen cut",
                ));
            }
            if agent.decision["revision"].as_u64() != Some(job.decision_revision) {
                return Err(CoreError::conflict(
                    "Editor decision snapshot changed before dispatch",
                ));
            }
            let profile = job.model_profile.clone().ok_or_else(|| {
                CoreError::new(
                    "model_unconfigured",
                    "no host-configured Editor profile was frozen for this job",
                    409,
                )
            })?;
            provider::validate_openai_profile(&profile).map_err(err)?;
            let worker_ids = self.selected_ids(&mut tx, &agent, "worker").await?;
            let editor_ids = self.selected_ids(&mut tx, &agent, "editor").await?;
            if worker_ids != editor_ids {
                return Err(CoreError::conflict(
                    "shared prefix requires the Worker and Editor to select the same CB view",
                ));
            }
            let mut cb_chain = Vec::new();
            for cb_id in &worker_ids {
                let cb = storage::get_object(&mut tx, cb_id, Some("cb")).await?;
                let body = cb["body"]
                    .as_str()
                    .ok_or_else(|| CoreError::invalid("CB body missing"))?;
                cb_chain.push(prefix::RenderedCb {
                    cb_id: cb_id.clone(),
                    messages: vec![json!({"role":"user","content":body})],
                });
            }
            let cut_id = agent
                .current_cut
                .clone()
                .unwrap_or_else(|| format!("genesis:{chain_id}"));
            let plan = prefix::PrefixPlan::freeze_with_instructions(
                prefix::PrefixPlanSpec {
                    model: required_str(&profile, "model")?.to_owned(),
                    provider_id: "configured-host-provider".into(),
                    cache_namespace: chain_id.into(),
                    chain_id: chain_id.into(),
                    context_id: job.context_id.clone(),
                    level: job.level,
                    cut_id: cut_id.clone(),
                    cb_chain,
                    projection_scope: "selected-shared-cb-view".into(),
                    projection_budget: profile["max_input_tokens"].as_u64().unwrap_or(1),
                    schema_version: "1".into(),
                    tool_version: "1".into(),
                },
                agent.protected.clone(),
            )
            .map_err(err)?;
            let visible_tools: Vec<String> = plan
                .common_tools()
                .map_err(err)?
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect();
            let editable_units: Vec<prefix::EditableUnit> = job.source_snapshot["units"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("frozen units missing"))?
                .iter()
                .map(|unit| {
                    Ok(prefix::EditableUnit {
                        unit_id: required_str(unit, "unit_id")?.into(),
                        source_id: job.source_id.clone(),
                        text: unit["text"]
                            .as_str()
                            .ok_or_else(|| CoreError::invalid("unit text missing"))?
                            .into(),
                        cb_id: None,
                    })
                })
                .collect::<Result<_>>()?;
            let auth = prefix::HostAuthorization::from_host(prefix::HostAuthorizationSpec {
                plan_digest: plan.digest().into(),
                actor: context.actor.clone(),
                chain_id: chain_id.into(),
                context_id: job.context_id.clone(),
                level: job.level,
                cut_id,
                cb_ids: worker_ids.clone(),
                scope_id: job.wb_id.clone(),
                mode: prefix::Mode::Editor,
                acl_epoch: 0,
                visible_tools,
                allowed_tools: vec!["sic_edit_blocks".into()],
                editor_scope: Some(prefix::EditorScope {
                    job_id: id.clone(),
                    wb_id: job.wb_id.clone(),
                    source_id: job.source_id.clone(),
                    read_only_cb_ids: worker_ids,
                    editable_units,
                    read_only_background: vec![],
                }),
            })
            .map_err(err)?;
            let i_views = json!({"current":agent.level(&job.context_id,job.level)?.instant,"decision_tree":agent.decision,
                "task_goal":job.frozen_editor_config["task_goal"],"task_constraints":job.frozen_editor_config["task_constraints"],
                "editor_instruction":job.frozen_editor_config["editor_prompt"],
                "edit_contract":"Return exactly one sic_edit_blocks call with ops array and pvi object. Unmentioned units default to KEEP. Operations are KEEP, DELETE, FLAG, REORDER, COMPRESS, REPLACE, ADD; use only listed unit IDs. REPLACE/COMPRESS require target and content, ADD requires content. P persists in CB, V becomes later WB, I is a replaceable named view. Preserve facts and tool outcomes."});
            let built = prefix::editor_request(&plan, &auth, &[], &i_views).map_err(err)?;
            let mut request = built.request;
            request
                .as_object_mut()
                .ok_or_else(|| CoreError::invalid("bad built request"))?
                .remove("model");
            apply_profile_reasoning(&profile, &mut request)?;
            surrogate_input_count(&request)?;
            let event_id = storage::uid("editor_run");
            storage::put_object(
                &mut tx,
                "editor_run",
                &event_id,
                &json!({"job_id":id,"claim_id":claim,"plan":plan,
                "prefix_digest":built.common_prefix_digest,"profile":profile,"started":now()}),
            )
            .await?;
            let held = agent
                .jobs
                .get_mut(&id)
                .ok_or_else(|| CoreError::invalid("job disappeared"))?;
            held.status = "inflight".into();
            held.claim_id = Some(claim.clone());
            agent.events.push(
                json!({"type":"editor_run_started","job_id":id,"record_id":event_id,"time":now()}),
            );
            let control_epoch = agent.control_epoch;
            let frozen_decision = agent.decision.clone();
            save_agent(&mut tx, &agent).await?;
            tx.commit().await?;
            (
                job,
                profile,
                request,
                built.common_prefix_digest,
                control_epoch,
                frozen_decision,
            )
        };

        let max_corrections = job.frozen_editor_config["editor_feedback"]["max_corrections"]
            .as_u64()
            .unwrap_or(2)
            .min(3);
        let feedback_chars = job.frozen_editor_config["editor_feedback"]["feedback_chars"]
            .as_u64()
            .unwrap_or(8192)
            .clamp(512, 32768) as usize;
        for attempt in 0..=max_corrections {
            let count = surrogate_input_count(&request)?;
            let captured = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
            let observed = captured.clone();
            let called = provider::chat_completion_observed(
                &profile,
                &request,
                provider::CallRole::Editor,
                &count,
                |name| (self.secret_resolver)(name),
                move |body| {
                    if let Ok(mut slot) = observed.lock() {
                        *slot = Some(body.to_vec());
                    }
                },
            )
            .await;
            let (candidate, raw, usage, error, retryable) = match called {
                Ok(result) => {
                    let validation = editable_editor_response(&result, &job, &frozen_decision);
                    match validation {
                        Ok(candidate) => (
                            Some(candidate),
                            Some(result.raw_response),
                            result.usage,
                            None,
                            false,
                        ),
                        Err(error) => (
                            None,
                            Some(result.raw_response),
                            result.usage,
                            Some(error),
                            true,
                        ),
                    }
                }
                Err(error) => {
                    let retryable = retryable_editor_output(&error);
                    (
                        None,
                        observed_body_value(&captured),
                        None,
                        Some(error),
                        retryable,
                    )
                }
            };
            let last = candidate.is_some() || !retryable || attempt == max_corrections;
            let (record_id, blocked) = {
                let mut tx = self.store.begin().await?;
                let mut agent = load_agent(&mut tx, chain_id, &context.actor).await?;
                let held = agent
                    .jobs
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| CoreError::invalid("job disappeared"))?;
                if held.status != "inflight" || held.claim_id.as_deref() != Some(claim.as_str()) {
                    return Err(CoreError::conflict(
                        "Editor claim changed while the provider was running",
                    ));
                }
                let blocked =
                    if agent.run_state != "running" || agent.control_epoch != control_epoch {
                        Some(CoreError::new(
                            "editor_interrupted",
                            "Editor result was not adopted after a state change",
                            409,
                        ))
                    } else if held.base_cut != agent.cut_seq {
                        Some(CoreError::conflict(
                            "Editor background changed while provider was running",
                        ))
                    } else {
                        match check_dependencies(&mut tx, &agent).await {
                            Err(error) => Some(error),
                            Ok(()) => self.check_board_access(&mut tx, &agent).await.err(),
                        }
                    };
                let aid = storage::uid("model_attempt");
                storage::put_object(&mut tx,"model_attempt",&aid,&json!({"job_id":id,"claim_id":claim,"attempt":attempt,
                    "request_digest":storage::digest(&request)?,"input_count":count.tokens,"counter_id":count.counter_id,
                    "raw_response":raw,"usage":usage,"validation_error":error,"interrupted":blocked.is_some(),"finished":now()})).await?;
                if let Some(candidate) = candidate.as_ref().filter(|_| blocked.is_none()) {
                    let rid = storage::uid("editor_response");
                    storage::put_object(&mut tx, "editor_response", &rid, candidate).await?;
                    let editor = required_str(&held.assignments[0], "editor")?;
                    let mut responses = Map::new();
                    responses.insert(editor.into(), candidate.clone());
                    let held = agent.jobs.get_mut(&id).unwrap();
                    held.responses = Value::Object(responses);
                    held.response_id = Some(rid);
                    held.candidate = Some(candidate.clone());
                    held.status = "ready".into();
                    held.claim_id = None;
                } else if last || blocked.is_some() {
                    let held = agent.jobs.get_mut(&id).unwrap();
                    held.status = "failed".into();
                    held.claim_id = None;
                }
                agent.events.push(
                    json!({"type":"editor_attempt","job_id":id,"record_id":aid,"time":now()}),
                );
                save_agent(&mut tx, &agent).await?;
                tx.commit().await?;
                (aid, blocked)
            };
            if let Some(blocked) = blocked {
                return Err(blocked);
            }
            if candidate.is_some() {
                return Ok(
                    json!({"ok":true,"operation":"edit.run","context":{"chain_id":chain_id,"actor":context.actor},
                    "result":{"job_id":id,"status":"ready","attempts":attempt+1,"last_attempt_id":record_id},
                    "evidence":{"provider_called":true,"physical_cache_hit":"unknown","common_prefix_digest":prefix_digest,
                                "token_count":"host-surrogate-not-provider-usage"}}),
                );
            }
            let error = error.unwrap_or_else(|| "Editor validation failed".into());
            if last {
                return Err(CoreError::new(
                    "editor_failed",
                    format!("Editor failed after {} attempt(s): {}", attempt + 1, error),
                    502,
                ));
            }
            let bounded: String = error.chars().take(feedback_chars).collect();
            request["messages"].as_array_mut().ok_or_else(||CoreError::invalid("request messages missing"))?
                .push(json!({"role":"user","content":format!("Host validation rejected the prior delivery: {bounded}. Correct it and call sic_edit_blocks once with valid ops and pvi; target only assigned units.")}));
        }
        Err(CoreError::new(
            "editor_failed",
            "Editor did not complete",
            502,
        ))
    }
}
