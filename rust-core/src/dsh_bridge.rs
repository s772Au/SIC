//! Authenticated DSH headless Chat bridge over the implemented Rust Core.
//!
//! Mount with `Router::nest("/v1", dsh_bridge::router(...))`. The caller must
//! provide an actor resolver backed by the host's authentication mechanism.
//! The bridge persists session history and turn receipts in the Rust Core
//! Store. In-flight work is never automatically replayed after an interruption.

#[path = "dsh.rs"]
mod wire;

use crate::{
    core::{Core, OperationContext},
    storage::{self, Store},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A host-owned authenticated actor resolver. It must validate authentication
/// before returning an actor; request JSON never supplies the actor identity.
pub type ActorResolver = Arc<dyn Fn(&HeaderMap) -> Result<String, String> + Send + Sync>;

/// Core operation boundary, injectable for deterministic tests. The production
/// adapter below delegates to `Core::operate`; tests use a fixed mock runtime.
pub trait DshRuntime: Send + Sync {
    fn operate(
        &self,
        actor: String,
        chain_id: Option<String>,
        operation: String,
        payload: Value,
        allow_model: bool,
    ) -> BoxFuture<Result<Value, String>>;
}

#[derive(Clone)]
pub struct CoreRuntime {
    core: Core,
}

impl CoreRuntime {
    pub fn new(core: Core) -> Self {
        Self { core }
    }
}

impl DshRuntime for CoreRuntime {
    fn operate(
        &self,
        actor: String,
        chain_id: Option<String>,
        operation: String,
        payload: Value,
        allow_model: bool,
    ) -> BoxFuture<Result<Value, String>> {
        let core = self.core.clone();
        Box::pin(async move {
            let context = OperationContext {
                actor,
                chain_id,
                allow_write: true,
                allow_model,
                allow_revoke: false,
            };
            core.operate(&operation, &context, &payload)
                .await
                // Only the fixed error code crosses this boundary; never the
                // Core error text, source content, or provider response.
                .map_err(|error| error.code.to_owned())
        })
    }
}

#[derive(Clone, Debug)]
pub struct DshBridgeConfig {
    pub model_alias: String,
    /// If DSH supplies max_tokens, it must exactly match this fixed host value.
    /// Per-request budget changes cannot be applied to the already configured Core.
    pub fixed_max_tokens: Option<u64>,
    /// Maximum Worker model calls across one DSH user turn, including tool continuations.
    pub max_worker_calls_per_turn: u32,
    /// Maximum external tool tasks emitted by any one Worker response.
    pub max_host_tasks_per_response: usize,
    /// Maximum harness tools admitted into a fixed chain catalog.
    pub max_harness_tools: usize,
    /// Optional fixed host choice forwarded through the Core profile. A DSH
    /// request may repeat this exact value, but cannot change or unset it.
    pub fixed_reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DshBridgeError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}

impl DshBridgeError {
    fn invalid(message: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "unsupported_request",
            message,
        }
    }
    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message,
        }
    }
    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "bridge_storage",
            message: "Bridge storage operation failed",
        }
    }
    fn core_failure() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "core_failed",
            message: "Core pipeline failed; this turn will not be retried automatically",
        }
    }
}

#[derive(Clone)]
pub struct DshBridge {
    store: Store,
    _store_guard: Arc<storage::StoreBridgeGuard>,
    runtime: Arc<dyn DshRuntime>,
    config: DshBridgeConfig,
    session_gates: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>,
        >,
    >,
}

#[derive(Clone)]
struct SessionRow {
    chain_id: Option<String>,
    pending_sources: Vec<String>,
    pending_tasks: Vec<PendingTask>,
    worker_calls: u32,
    host_task_count: usize,
    catalog_changed: bool,
    catalog_fingerprint: String,
}

#[derive(Clone, Debug)]
struct PendingTask {
    task_id: String,
    call_id: String,
    tool_name: String,
    arguments: Value,
}

struct TurnOutput {
    sse: String,
    history: Vec<Value>,
    pending_sources: Vec<String>,
    pending_tasks: Vec<PendingTask>,
    worker_calls: u32,
    host_task_count: usize,
    has_outbound_calls: bool,
}

fn host_tools_configuration(
    actor: &str,
    tools: &Value,
    max_tools: usize,
) -> Result<(String, Value, Vec<Value>), DshBridgeError> {
    let entries = tools
        .as_array()
        .ok_or_else(|| DshBridgeError::invalid("DSH tools must be an array"))?;
    if entries.len() > max_tools {
        return Err(DshBridgeError::invalid(
            "DSH tool table exceeds the host limit",
        ));
    }
    let mut names = std::collections::HashSet::new();
    let reserved_sic_names = Core::common_tool_names();
    let mut descriptors = Vec::with_capacity(entries.len());
    let mut rows = Vec::with_capacity(entries.len());
    let mut grants = Vec::with_capacity(entries.len());
    let mut total_tool_bytes = 0usize;
    for entry in entries {
        let object = entry
            .as_object()
            .ok_or_else(|| DshBridgeError::invalid("DSH tool schema must be an object"))?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "function"))
            || object.get("type").and_then(Value::as_str) != Some("function")
        {
            return Err(DshBridgeError::invalid(
                "only native OpenAI function tools are supported",
            ));
        }
        let function = entry
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| DshBridgeError::invalid("DSH function tool is incomplete"))?;
        if function
            .keys()
            .any(|key| !matches!(key.as_str(), "name" | "description" | "parameters"))
        {
            return Err(DshBridgeError::invalid(
                "DSH function tool has unsupported controls",
            ));
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 128)
            .ok_or_else(|| DshBridgeError::invalid("DSH tool name is invalid"))?;
        if reserved_sic_names.iter().any(|reserved| reserved == name) {
            return Err(DshBridgeError::invalid(
                "DSH tool name collides with reserved SIC operations",
            ));
        }
        if !names.insert(name.to_owned()) {
            return Err(DshBridgeError::invalid("DSH tool names must be unique"));
        }
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let schema = function
            .get("parameters")
            .filter(|schema| schema.is_object())
            .ok_or_else(|| {
                DshBridgeError::invalid("DSH tool parameters must be a JSON Schema object")
            })?;
        let descriptor_bytes = storage::json_text(&json!({
            "name":name,
            "description":description,
            "schema":schema
        }))
        .map_err(|_| DshBridgeError::internal())?
        .len();
        total_tool_bytes = total_tool_bytes.saturating_add(descriptor_bytes);
        if total_tool_bytes > 1_048_576 {
            return Err(DshBridgeError::invalid(
                "DSH tool catalog metadata exceeds the host size limit",
            ));
        }
        let descriptor = json!({"name":name,"description":description,"schema":schema});
        descriptors.push(descriptor.clone());
        rows.push(json!({"descriptor":descriptor,"host_fixed":false}));
        grants.push(json!({"principal":actor,"tool_name":name}));
    }
    let fingerprint = storage::digest(tools).map_err(|_| DshBridgeError::internal())?;
    let configuration = if entries.is_empty() {
        Value::Null
    } else {
        json!({
            "acl":{"discover":grants,"expand":grants.clone(),"execute":grants.clone()},
            "tools":rows
        })
    };
    Ok((fingerprint, configuration, descriptors))
}

async fn load_pending_tasks(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &str,
    session: &str,
) -> Result<Vec<PendingTask>, DshBridgeError> {
    let rows = sqlx::query("SELECT call_id,task_id,tool_name,arguments FROM dsh_bridge_tool_tasks WHERE actor=? AND session_key=? AND task_state='issued' ORDER BY created,call_id")
        .bind(actor).bind(session).fetch_all(&mut **tx).await.map_err(|_| DshBridgeError::internal())?;
    rows.into_iter()
        .map(|row| {
            let arguments_text: String = row
                .try_get("arguments")
                .map_err(|_| DshBridgeError::internal())?;
            Ok(PendingTask {
                task_id: row
                    .try_get("task_id")
                    .map_err(|_| DshBridgeError::internal())?,
                call_id: row
                    .try_get("call_id")
                    .map_err(|_| DshBridgeError::internal())?,
                tool_name: row
                    .try_get("tool_name")
                    .map_err(|_| DshBridgeError::internal())?,
                arguments: serde_json::from_str(&arguments_text)
                    .map_err(|_| DshBridgeError::internal())?,
            })
        })
        .collect()
}

enum ClaimedTurn {
    Replay(Vec<u8>),
    New(SessionRow, Vec<Value>),
}

impl DshBridge {
    async fn lock_session(
        &self,
        actor: &str,
        session: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, DshBridgeError> {
        let gate_key =
            storage::digest(&json!([actor, session])).map_err(|_| DshBridgeError::internal())?;
        let gate = {
            let mut gates = self
                .session_gates
                .lock()
                .map_err(|_| DshBridgeError::internal())?;
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(existing) = gates.get(&gate_key).and_then(std::sync::Weak::upgrade) {
                existing
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(gate_key, Arc::downgrade(&gate));
                gate
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(300), gate.lock_owned())
            .await
            .map_err(|_| {
                DshBridgeError::conflict(
                    "session_wait_timeout",
                    "DSH session wait exceeded the host limit",
                )
            })
    }

    pub async fn new(
        store: Store,
        runtime: Arc<dyn DshRuntime>,
        config: DshBridgeConfig,
    ) -> Result<Self, DshBridgeError> {
        if config.model_alias.trim().is_empty()
            || config.fixed_max_tokens.is_some_and(|tokens| tokens == 0)
            || config.max_worker_calls_per_turn == 0
            || config.max_worker_calls_per_turn > 32
            || config.max_host_tasks_per_response == 0
            || config.max_host_tasks_per_response > 16
            || config.max_harness_tools == 0
            || config.max_harness_tools > 4096
            || config
                .fixed_reasoning_effort
                .as_deref()
                .is_some_and(|effort| {
                    !matches!(effort, "low" | "medium" | "high" | "xhigh" | "max")
                })
        {
            return Err(DshBridgeError::invalid(
                "invalid fixed DSH route configuration",
            ));
        }
        // Recovery initialization must only run while this process owns the
        // exclusive database guard. Otherwise another live bridge could be
        // mistaken for an abandoned process and its turns marked indeterminate.
        let store_guard = store.acquire_dsh_bridge_guard().map_err(|_| {
            DshBridgeError::conflict(
                "bridge_instance_active",
                "Another DSH bridge instance already owns this database",
            )
        })?;
        let bridge = Self {
            store,
            _store_guard: Arc::new(store_guard),
            runtime,
            config,
            session_gates: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };
        bridge.initialize().await?;
        Ok(bridge)
    }

    async fn initialize(&self) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dsh_bridge_sessions(\
                actor TEXT NOT NULL,session_key TEXT NOT NULL,model_alias TEXT NOT NULL,\
                chain_id TEXT,history TEXT NOT NULL,pending_sources TEXT NOT NULL,\
                active_fingerprint TEXT,tools_fingerprint TEXT NOT NULL DEFAULT '',\
                worker_calls INTEGER NOT NULL DEFAULT 0,host_task_count INTEGER NOT NULL DEFAULT 0,\
                PRIMARY KEY(actor,session_key))",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        let columns = sqlx::query("PRAGMA table_info(dsh_bridge_sessions)")
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let has_column = |name: &str| {
            columns.iter().any(|row| {
                row.try_get::<String, _>("name")
                    .is_ok_and(|column| column == name)
            })
        };
        for (name, declaration) in [
            ("tools_fingerprint", "TEXT NOT NULL DEFAULT ''"),
            ("worker_calls", "INTEGER NOT NULL DEFAULT 0"),
            ("host_task_count", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !has_column(name) {
                sqlx::query(&format!(
                    "ALTER TABLE dsh_bridge_sessions ADD COLUMN {name} {declaration}"
                ))
                .execute(&mut *tx)
                .await
                .map_err(|_| DshBridgeError::internal())?;
            }
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dsh_bridge_tool_tasks(\
                actor TEXT NOT NULL,session_key TEXT NOT NULL,call_id TEXT NOT NULL,\
                task_id TEXT NOT NULL,tool_name TEXT NOT NULL,arguments TEXT NOT NULL,\
                task_state TEXT NOT NULL,result_fingerprint TEXT,created INTEGER NOT NULL,\
                PRIMARY KEY(actor,session_key,call_id),UNIQUE(actor,session_key,task_id))",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dsh_bridge_turns(\
                actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,\
                state TEXT NOT NULL,phase TEXT NOT NULL,progress TEXT NOT NULL,\
                response_sse TEXT,error_code TEXT,created INTEGER NOT NULL,\
                PRIMARY KEY(actor,session_key,fingerprint))",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS dsh_bridge_retirements(\
            actor TEXT NOT NULL,session_key TEXT NOT NULL,chain_id TEXT,\
            fingerprint TEXT NOT NULL,original_state TEXT NOT NULL,original_phase TEXT NOT NULL,\
            reason TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(actor,session_key))",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        // A process restart cannot tell whether an interrupted Core model call
        // completed. Preserve its receipt as indeterminate and block replay.
        sqlx::query("UPDATE dsh_bridge_turns SET state='indeterminate',error_code='bridge_restarted' WHERE state='running'")
            .execute(&mut *tx)
            .await
            .map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    /// Handle a DSH request for an actor already authenticated by the host.
    /// Returns the complete buffered SSE body. Duplicate delivered turns replay
    /// byte-for-byte; any failed or uncertain turn remains blocked.
    pub async fn handle(
        &self,
        actor: &str,
        headers: &HeaderMap,
        body: &Value,
    ) -> Result<Vec<u8>, DshBridgeError> {
        if actor.trim().is_empty() {
            return Err(DshBridgeError {
                status: StatusCode::UNAUTHORIZED,
                code: "unauthorized",
                message: "Host authentication required",
            });
        }
        let request = wire::DshChatRequest::parse(headers, body, &self.config.model_alias)
            .map_err(|_| DshBridgeError::invalid("DSH Chat request is malformed or unsupported"))?;
        // DSH can dispatch two calls for the same session concurrently. Hold
        // a per-session gate across the whole turn, then recheck history and
        // receipts. No network or database lock is held while waiting.
        let _permit = self.lock_session(actor, &request.session_id).await?;
        if request
            .reasoning_effort
            .as_deref()
            .is_some_and(|effort| self.config.fixed_reasoning_effort.as_deref() != Some(effort))
        {
            return Err(DshBridgeError::invalid(
                "reasoning_effort must match the fixed host route",
            ));
        }
        if request.max_tokens.is_some() && request.max_tokens != self.config.fixed_max_tokens {
            return Err(DshBridgeError::invalid(
                "max_tokens must match the fixed host route",
            ));
        }
        let (tools_fingerprint, configure_payload, harness_tools) =
            host_tools_configuration(actor, &request.tools, self.config.max_harness_tools)?;
        let fingerprint = storage::digest(&json!({
            "model": request.model_alias,
            "messages": request.messages,
            "max_tokens": request.max_tokens,
            "include_usage": request.include_usage,
            "fixed_reasoning_effort": self.config.fixed_reasoning_effort,
            "tools_fingerprint": tools_fingerprint
        }))
        .map_err(|_| DshBridgeError::internal())?;
        let (session, additions) = match self
            .claim_turn(actor, &request, &fingerprint, &tools_fingerprint)
            .await?
        {
            ClaimedTurn::Replay(bytes) => return Ok(bytes),
            ClaimedTurn::New(session, additions) => (session, additions),
        };
        let result = self
            .run_turn(
                actor,
                &request,
                &fingerprint,
                session,
                additions,
                configure_payload,
                harness_tools,
            )
            .await;
        match result {
            Ok(output) => {
                self.deliver(actor, &request.session_id, &fingerprint, &output)
                    .await?;
                Ok(output.sse.into_bytes())
            }
            Err((error, uncertain)) => {
                self.fail_turn(
                    actor,
                    &request.session_id,
                    &fingerprint,
                    error.code,
                    uncertain,
                )
                .await?;
                Err(error)
            }
        }
    }

    async fn claim_turn(
        &self,
        actor: &str,
        request: &wire::DshChatRequest,
        fingerprint: &str,
        tools_fingerprint: &str,
    ) -> Result<ClaimedTurn, DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let retired: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM dsh_bridge_retirements WHERE actor=? AND session_key=?",
        )
        .bind(actor)
        .bind(&request.session_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        if retired.is_some() {
            return Err(DshBridgeError::conflict(
                "session_retired",
                "DSH session was retired; its work will not be replayed",
            ));
        }
        if let Some(row) = sqlx::query("SELECT state,response_sse,error_code FROM dsh_bridge_turns WHERE actor=? AND session_key=? AND fingerprint=?")
            .bind(actor).bind(&request.session_id).bind(fingerprint).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?
        {
            let state: String = row.try_get("state").map_err(|_| DshBridgeError::internal())?;
            if state == "delivered" {
                let sse: String = row.try_get("response_sse").map_err(|_| DshBridgeError::internal())?;
                tx.commit().await.map_err(|_| DshBridgeError::internal())?;
                return Ok(ClaimedTurn::Replay(sse.into_bytes()));
            }
            tx.commit().await.map_err(|_| DshBridgeError::internal())?;
            return Err(DshBridgeError::conflict(
                if state == "indeterminate" { "turn_indeterminate" } else { "turn_incomplete" },
                "This DSH turn has an existing failed or uncertain receipt and will not be replayed",
            ));
        }

        sqlx::query("INSERT OR IGNORE INTO dsh_bridge_sessions(actor,session_key,model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count) VALUES(?,?,?,NULL,'[]','[]',NULL,?,0,0)")
            .bind(actor).bind(&request.session_id).bind(&request.model_alias).bind(tools_fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let row = sqlx::query("SELECT model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count FROM dsh_bridge_sessions WHERE actor=? AND session_key=?")
            .bind(actor).bind(&request.session_id).fetch_one(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let model_alias: String = row
            .try_get("model_alias")
            .map_err(|_| DshBridgeError::internal())?;
        if model_alias != request.model_alias {
            return Err(DshBridgeError::conflict(
                "model_alias_conflict",
                "DSH session is bound to a different fixed model alias",
            ));
        }
        let stored_tools_fingerprint: String = row
            .try_get("tools_fingerprint")
            .map_err(|_| DshBridgeError::internal())?;
        let catalog_changed = stored_tools_fingerprint != tools_fingerprint;
        let active: Option<String> = row
            .try_get("active_fingerprint")
            .map_err(|_| DshBridgeError::internal())?;
        if active.is_some() {
            return Err(DshBridgeError::conflict(
                "session_busy",
                "DSH session has an unresolved turn",
            ));
        }
        let history_text: String = row
            .try_get("history")
            .map_err(|_| DshBridgeError::internal())?;
        let history: Vec<Value> =
            serde_json::from_str(&history_text).map_err(|_| DshBridgeError::internal())?;
        let pending_tasks = load_pending_tasks(&mut tx, actor, &request.session_id).await?;
        let additions = history_delta(&history, &request.messages, &pending_tasks)?;
        let chain_id: Option<String> = row
            .try_get("chain_id")
            .map_err(|_| DshBridgeError::internal())?;
        let pending_text: String = row
            .try_get("pending_sources")
            .map_err(|_| DshBridgeError::internal())?;
        let pending_sources: Vec<String> =
            serde_json::from_str(&pending_text).map_err(|_| DshBridgeError::internal())?;
        let worker_calls: i64 = row
            .try_get("worker_calls")
            .map_err(|_| DshBridgeError::internal())?;
        let host_task_count: i64 = row
            .try_get("host_task_count")
            .map_err(|_| DshBridgeError::internal())?;
        let (worker_calls, host_task_count) = if pending_tasks.is_empty() {
            (0, 0)
        } else {
            (worker_calls.max(0) as u32, host_task_count.max(0) as usize)
        };
        sqlx::query("INSERT INTO dsh_bridge_turns(actor,session_key,fingerprint,state,phase,progress,response_sse,error_code,created) VALUES(?,?,?,'running','admitted','{}',NULL,NULL,?)")
            .bind(actor).bind(&request.session_id).bind(fingerprint).bind(now() as i64).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        sqlx::query(
            "UPDATE dsh_bridge_sessions SET active_fingerprint=? WHERE actor=? AND session_key=?",
        )
        .bind(fingerprint)
        .bind(actor)
        .bind(&request.session_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(ClaimedTurn::New(
            SessionRow {
                chain_id,
                pending_sources,
                pending_tasks,
                worker_calls,
                host_task_count,
                catalog_changed,
                catalog_fingerprint: tools_fingerprint.to_owned(),
            },
            additions,
        ))
    }

    async fn run_turn(
        &self,
        actor: &str,
        request: &wire::DshChatRequest,
        fingerprint: &str,
        mut session: SessionRow,
        additions: Vec<Value>,
        configure_payload: Value,
        harness_tools: Vec<Value>,
    ) -> Result<TurnOutput, (DshBridgeError, bool)> {
        let mut progress = json!({"worker_rounds":[],"tool_completions":[]});
        let mut chain_id = session.chain_id.clone();
        if chain_id.is_none() {
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "chain_create_started",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;
            let created = self
                .operation(actor, None, "chain.create", json!({}), false)
                .await
                .map_err(|e| (e, false))?;
            chain_id = created["result"]["chain_id"].as_str().map(str::to_owned);
            let Some(id) = chain_id.clone() else {
                return Err((DshBridgeError::core_failure(), false));
            };
            session.chain_id = Some(id.clone());
            progress["chain_id"] = json!(id);
            self.persist_session_chain(actor, &request.session_id, &id, fingerprint, &progress)
                .await
                .map_err(|e| (e, false))?;
            if !configure_payload.is_null() {
                self.set_phase(
                    actor,
                    &request.session_id,
                    fingerprint,
                    "tools_configure_started",
                    &progress,
                )
                .await
                .map_err(|e| (e, false))?;
                self.operation(
                    actor,
                    Some(&id),
                    "tools.configure",
                    configure_payload.clone(),
                    false,
                )
                .await
                .map_err(|e| (e, false))?;
                self.set_phase(
                    actor,
                    &request.session_id,
                    fingerprint,
                    "tools_configured",
                    &progress,
                )
                .await
                .map_err(|e| (e, false))?;
            }
        }
        let chain = chain_id.as_deref().expect("chain id saved");
        let mut pending_sources = session.pending_sources.clone();
        let mut worker_calls = session.worker_calls;
        let mut host_task_count = session.host_task_count;
        if session.pending_tasks.is_empty() {
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "source_ingest_started",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;
            let ingested = self
                .operation(
                    actor,
                    Some(chain),
                    "source.ingest",
                    json!({
                        "idempotency_key":format!("dsh:{fingerprint}:ingest"),
                        "request":{"messages":additions}
                    }),
                    false,
                )
                .await
                .map_err(|e| (e, false))?;
            let source_id = ingested["result"]["source_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or((DshBridgeError::core_failure(), false))?;
            pending_sources.push(source_id.clone());
            progress["ingested_source_id"] = json!(source_id);
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "source_ingested",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;

            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "wb_form_started",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;
            // `wb.form` may combine prior Worker output and the new harness
            // message. Its root is the earliest pending source, not necessarily
            // the source just ingested on this turn.
            let expected_wb_source = pending_sources
                .first()
                .cloned()
                .ok_or((DshBridgeError::core_failure(), false))?;
            let formed = self
                .operation(
                    actor,
                    Some(chain),
                    "wb.form",
                    json!({
                        "source_ids":pending_sources,"reason":"dsh-turn"
                    }),
                    false,
                )
                .await
                .map_err(|e| (e, false))?;
            let wb_id = formed["result"]["wb_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or((DshBridgeError::core_failure(), false))?;
            let wb_source_id = formed["result"]["source_id"]
                .as_str()
                .filter(|source| *source == expected_wb_source)
                .map(str::to_owned)
                .ok_or((DshBridgeError::core_failure(), false))?;
            progress["wb_id"] = json!(wb_id);
            progress["wb_source_id"] = json!(wb_source_id);
            pending_sources.clear();

            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "edit_prepare_started",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;
            let prepared = self
                .operation(
                    actor,
                    Some(chain),
                    "edit.prepare",
                    json!({"source_id":wb_source_id}),
                    false,
                )
                .await
                .map_err(|e| (e, false))?;
            let job_id = prepared["result"]["job_id"]
                .as_str()
                .map(str::to_owned)
                .ok_or((DshBridgeError::core_failure(), false))?;
            progress["job_id"] = json!(job_id);
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "editor_model_started",
                &progress,
            )
            .await
            .map_err(|e| (e, false))?;
            let edited = self
                .operation(
                    actor,
                    Some(chain),
                    "edit.run",
                    json!({"job_id":job_id}),
                    true,
                )
                .await
                .map_err(|e| (e, true))?;
            if edited["result"]["status"].as_str() != Some("ready") {
                return Err((DshBridgeError::core_failure(), true));
            }
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "cut_read_started",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
            let chain_state = self
                .operation(actor, Some(chain), "chain.get", json!({}), false)
                .await
                .map_err(|e| (e, true))?;
            let expected_cut = chain_state["result"]["cut_seq"]
                .as_u64()
                .ok_or((DshBridgeError::core_failure(), true))?;
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "cut_publish_started",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
            let published = self
                .operation(
                    actor,
                    Some(chain),
                    "cut.publish",
                    json!({
                        "job_id":job_id,"expected_cut":expected_cut,
                        "idempotency_key":format!("dsh:{fingerprint}:publish")
                    }),
                    false,
                )
                .await
                .map_err(|e| (e, true))?;
            progress["published_cut"] = published["result"]["cut_id"].clone();
        } else {
            let additions_by_id: std::collections::HashMap<&str, &Value> = additions
                .iter()
                .filter_map(|message| Some((message.get("tool_call_id")?.as_str()?, message)))
                .collect();
            if additions_by_id.len() != session.pending_tasks.len() {
                return Err((
                    DshBridgeError::conflict(
                        "tool_result_duplicate",
                        "DSH tool results are duplicated",
                    ),
                    false,
                ));
            }
            for task in &session.pending_tasks {
                let message = additions_by_id.get(task.call_id.as_str()).ok_or((
                    DshBridgeError::conflict(
                        "tool_result_unmatched",
                        "DSH result has no issued task",
                    ),
                    false,
                ))?;
                let content = message["content"].as_str().ok_or((
                    DshBridgeError::invalid("DSH tool result must be plain text"),
                    false,
                ))?;
                let result = Value::String(content.to_owned());
                self.set_phase(
                    actor,
                    &request.session_id,
                    fingerprint,
                    "tool_complete_started",
                    &progress,
                )
                .await
                .map_err(|e| (e, true))?;
                let completed = self.operation(
                    actor,
                    Some(chain),
                    "tool.complete",
                    json!({
                        "task_id":task.task_id,
                        "claim_id":stable_tool_claim_id(actor, &request.session_id, &task.task_id)
                            .map_err(|e| (e, true))?,
                        "succeeded":Value::Null,"result":result,"delivery":"received"
                    }),
                    false,
                )
                .await
                .map_err(|e| (e, true))?;
                if completed["result"]["status"].as_str() != Some("delivered_unattested") {
                    return Err((
                        DshBridgeError::conflict(
                            "tool_delivery_unconfirmed",
                            "Core did not confirm receipt of the DSH tool output",
                        ),
                        true,
                    ));
                }
                let result_fingerprint =
                    storage::digest(&json!({"succeeded":null,"result":result}))
                        .map_err(|_| (DshBridgeError::internal(), true))?;
                self.mark_task_completed(
                    actor,
                    &request.session_id,
                    &task.call_id,
                    &result_fingerprint,
                )
                .await
                .map_err(|e| (e, true))?;
                progress["tool_completions"].as_array_mut().unwrap().push(json!({
                    "call_id":task.call_id,"task_id":task.task_id,"result_fingerprint":result_fingerprint
                }));
            }
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "tool_results_completed",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
        }

        // The authenticated DSH host may change its available tool catalog.
        // Resolve outstanding issued tasks first; then atomically reconcile
        // ACL/schema without modifying the fixed SIC common tool registry.
        if session.catalog_changed {
            let payload = if configure_payload.is_null() {
                json!({"acl":{"discover":[],"expand":[],"execute":[]},"tools":[]})
            } else {
                configure_payload
            };
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "tools_sync_started",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
            self.operation(actor, Some(chain), "tools.sync", payload, false)
                .await
                .map_err(|e| (e, true))?;
            let mut tx = self
                .store
                .begin()
                .await
                .map_err(|_| (DshBridgeError::internal(), true))?;
            let changed=sqlx::query("UPDATE dsh_bridge_sessions SET tools_fingerprint=? WHERE actor=? AND session_key=? AND active_fingerprint=?")
                .bind(&session.catalog_fingerprint).bind(actor).bind(&request.session_id).bind(fingerprint)
                .execute(&mut *tx).await.map_err(|_|(DshBridgeError::internal(),true))?;
            if changed.rows_affected() != 1 {
                return Err((
                    DshBridgeError::conflict(
                        "session_changed",
                        "DSH session changed during catalog sync",
                    ),
                    true,
                ));
            }
            tx.commit()
                .await
                .map_err(|_| (DshBridgeError::internal(), true))?;
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "tools_synced",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
        }

        loop {
            if worker_calls >= self.config.max_worker_calls_per_turn {
                return Err((
                    DshBridgeError::conflict(
                        "worker_round_limit",
                        "DSH Worker continuation limit reached",
                    ),
                    true,
                ));
            }
            worker_calls += 1;
            self.persist_workflow_counts(
                actor,
                &request.session_id,
                fingerprint,
                worker_calls,
                host_task_count,
            )
            .await
            .map_err(|e| (e, true))?;
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "worker_model_started",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;
            let worker = self
                .operation(actor, Some(chain), "worker.run", json!({}), true)
                .await
                .map_err(|e| (e, true))?;
            let result = &worker["result"];
            let calls = result
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let tool_results = result
                .get("tool_results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let host_tasks = result
                .get("host_tasks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(source_id) = result.get("source_id").and_then(Value::as_str) {
                if !pending_sources.iter().any(|id| id == source_id) {
                    pending_sources.push(source_id.to_owned());
                }
            }
            validate_worker_round(&calls, &tool_results, &host_tasks)
                .map_err(|_| (DshBridgeError::core_failure(), true))?;
            progress["worker_rounds"].as_array_mut().unwrap().push(json!({
                "attempt_id":result.get("attempt_id"),"source_id":result.get("source_id"),
                "tool_calls":calls,"tool_results":tool_results,"host_task_ids":host_tasks.iter().map(|task|task["task_id"].clone()).collect::<Vec<_>>()
            }));
            self.set_phase(
                actor,
                &request.session_id,
                fingerprint,
                "worker_result_received",
                &progress,
            )
            .await
            .map_err(|e| (e, true))?;

            if !host_tasks.is_empty() {
                if host_tasks.len() > self.config.max_host_tasks_per_response {
                    return Err((
                        DshBridgeError::conflict(
                            "host_task_limit",
                            "Worker response exceeds the host task limit",
                        ),
                        true,
                    ));
                }
                let new_tasks =
                    validate_host_tasks(actor, chain, &host_tasks, &calls, &harness_tools)
                        .map_err(|_| (DshBridgeError::core_failure(), true))?;
                let total_task_limit = self
                    .config
                    .max_host_tasks_per_response
                    .saturating_mul(self.config.max_worker_calls_per_turn as usize);
                let next_host_task_count = host_task_count.saturating_add(new_tasks.len());
                if next_host_task_count > total_task_limit {
                    return Err((
                        DshBridgeError::conflict(
                            "host_task_limit",
                            "DSH turn exceeds the cumulative host task limit",
                        ),
                        true,
                    ));
                }
                host_task_count = next_host_task_count;
                // Persist claims before exposing any tool call to DSH. A
                // lost response is ambiguous and never triggers re-dispatch.
                for task in &new_tasks {
                    let claim_id = stable_tool_claim_id(actor, &request.session_id, &task.task_id)
                        .map_err(|error| (error, true))?;
                    let claimed = self
                        .operation(
                            actor,
                            Some(chain),
                            "tool.claim",
                            json!({"task_id":task.task_id,"claim_id":claim_id}),
                            false,
                        )
                        .await
                        .map_err(|error| (error, true))?;
                    if claimed["result"]["status"] != "claimed"
                        || claimed.pointer("/evidence/replayed") == Some(&Value::Bool(true))
                    {
                        return Err((
                            DshBridgeError::conflict(
                                "tool_claim_replayed",
                                "External task claim already existed; dispatch is not safe to repeat",
                            ),
                            true,
                        ));
                    }
                }
                let external_calls: Vec<Value> = new_tasks
                    .iter()
                    .map(|task| {
                        json!({
                            "id":task.call_id,"type":"function","function":{"name":task.tool_name,
                            "arguments":serde_json::to_string(&task.arguments).unwrap_or_default()}
                        })
                    })
                    .collect();
                if external_calls
                    .iter()
                    .any(|call| call["function"]["arguments"].as_str() == Some(""))
                {
                    return Err((DshBridgeError::core_failure(), true));
                }
                self.persist_workflow_counts(
                    actor,
                    &request.session_id,
                    fingerprint,
                    worker_calls,
                    host_task_count,
                )
                .await
                .map_err(|e| (e, true))?;
                let message = json!({"role":"assistant","content":result.get("content").cloned().unwrap_or(Value::Null),"tool_calls":external_calls});
                let reply = chat_reply(request, message, "tool_calls");
                let sse = encode_reply(&reply, request.include_usage)?;
                let mut history = request.messages.clone();
                history.push(reply["choices"][0]["message"].clone());
                return Ok(TurnOutput {
                    sse,
                    history,
                    pending_sources,
                    pending_tasks: new_tasks,
                    worker_calls,
                    host_task_count,
                    has_outbound_calls: true,
                });
            }
            if !calls.is_empty() {
                continue;
            }
            let content = result
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or((DshBridgeError::core_failure(), true))?;
            let message = json!({"role":"assistant","content":content});
            let reply = chat_reply(request, message, "stop");
            let sse = encode_reply(&reply, request.include_usage)?;
            let mut history = request.messages.clone();
            history.push(reply["choices"][0]["message"].clone());
            return Ok(TurnOutput {
                sse,
                history,
                pending_sources,
                pending_tasks: Vec::new(),
                worker_calls: 0,
                host_task_count: 0,
                has_outbound_calls: false,
            });
        }
    }

    async fn persist_workflow_counts(
        &self,
        actor: &str,
        session: &str,
        fingerprint: &str,
        worker_calls: u32,
        host_task_count: usize,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        sqlx::query("UPDATE dsh_bridge_sessions SET worker_calls=?,host_task_count=? WHERE actor=? AND session_key=? AND active_fingerprint=?")
            .bind(i64::from(worker_calls)).bind(i64::try_from(host_task_count).unwrap_or(i64::MAX))
            .bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    async fn mark_task_completed(
        &self,
        actor: &str,
        session: &str,
        call_id: &str,
        fingerprint: &str,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let changed = sqlx::query("UPDATE dsh_bridge_tool_tasks SET task_state='completed',result_fingerprint=? WHERE actor=? AND session_key=? AND call_id=? AND task_state='issued'")
            .bind(fingerprint).bind(actor).bind(session).bind(call_id).execute(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        if changed.rows_affected() != 1 {
            return Err(DshBridgeError::conflict(
                "tool_task_state",
                "host task mapping is no longer pending",
            ));
        }
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    async fn operation(
        &self,
        actor: &str,
        chain_id: Option<&str>,
        operation: &str,
        payload: Value,
        allow_model: bool,
    ) -> Result<Value, DshBridgeError> {
        self.runtime
            .operate(
                actor.to_owned(),
                chain_id.map(str::to_owned),
                operation.to_owned(),
                payload,
                allow_model,
            )
            .await
            .map_err(|error| match error.as_str() {
                "protected_prefix_conflict" => DshBridgeError::conflict(
                    "protected_prefix_conflict",
                    "Harness instructions differ from the Agent's immutable protected prefix",
                ),
                _ => DshBridgeError::core_failure(),
            })
    }

    async fn set_phase(
        &self,
        actor: &str,
        session: &str,
        fingerprint: &str,
        phase: &str,
        progress: &Value,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        sqlx::query("UPDATE dsh_bridge_turns SET phase=?,progress=? WHERE actor=? AND session_key=? AND fingerprint=? AND state='running'")
            .bind(phase).bind(storage::json_text(progress).map_err(|_| DshBridgeError::internal())?).bind(actor).bind(session).bind(fingerprint)
            .execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    async fn persist_session_chain(
        &self,
        actor: &str,
        session: &str,
        chain_id: &str,
        fingerprint: &str,
        progress: &Value,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        sqlx::query("UPDATE dsh_bridge_sessions SET chain_id=? WHERE actor=? AND session_key=? AND active_fingerprint=?")
            .bind(chain_id).bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        sqlx::query("UPDATE dsh_bridge_turns SET phase='chain_created',progress=? WHERE actor=? AND session_key=? AND fingerprint=? AND state='running'")
            .bind(storage::json_text(progress).map_err(|_| DshBridgeError::internal())?).bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    async fn deliver(
        &self,
        actor: &str,
        session: &str,
        fingerprint: &str,
        output: &TurnOutput,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let session_update = sqlx::query("UPDATE dsh_bridge_sessions SET history=?,pending_sources=?,active_fingerprint=NULL,worker_calls=?,host_task_count=? WHERE actor=? AND session_key=? AND active_fingerprint=?")
            .bind(storage::json_text(&json!(output.history)).map_err(|_| DshBridgeError::internal())?)
            .bind(storage::json_text(&json!(output.pending_sources)).map_err(|_| DshBridgeError::internal())?)
            .bind(i64::from(output.worker_calls)).bind(i64::try_from(output.host_task_count).unwrap_or(i64::MAX))
            .bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        if session_update.rows_affected() != 1 {
            return Err(DshBridgeError::conflict(
                "session_changed",
                "DSH session changed before the turn could be delivered",
            ));
        }
        for task in &output.pending_tasks {
            sqlx::query("INSERT INTO dsh_bridge_tool_tasks(actor,session_key,call_id,task_id,tool_name,arguments,task_state,result_fingerprint,created) VALUES(?,?,?,?,?,?,'issued',NULL,?)")
                .bind(actor).bind(session).bind(&task.call_id).bind(&task.task_id).bind(&task.tool_name)
                .bind(storage::json_text(&task.arguments).map_err(|_|DshBridgeError::internal())?)
                .bind(now() as i64).execute(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        }
        let state = if output.has_outbound_calls {
            "tool_issued"
        } else {
            "delivered"
        };
        let cached_sse = (!output.has_outbound_calls).then_some(output.sse.as_str());
        let phase = if output.has_outbound_calls {
            "tool_issued"
        } else {
            "delivered"
        };
        let turn_update = sqlx::query("UPDATE dsh_bridge_turns SET state=?,phase=?,response_sse=?,error_code=NULL WHERE actor=? AND session_key=? AND fingerprint=? AND state='running'")
            .bind(state).bind(phase).bind(cached_sse).bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        if turn_update.rows_affected() != 1 {
            return Err(DshBridgeError::conflict(
                "turn_changed",
                "DSH turn changed before the response could be delivered",
            ));
        }
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    async fn fail_turn(
        &self,
        actor: &str,
        session: &str,
        fingerprint: &str,
        code: &'static str,
        uncertain: bool,
    ) -> Result<(), DshBridgeError> {
        let state = if uncertain { "indeterminate" } else { "failed" };
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let result = sqlx::query("UPDATE dsh_bridge_turns SET state=?,error_code=? WHERE actor=? AND session_key=? AND fingerprint=? AND state='running'")
            .bind(state).bind(code).bind(actor).bind(session).bind(fingerprint).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        if result.rows_affected() != 1 {
            return Err(DshBridgeError::conflict(
                "turn_changed",
                "DSH turn changed before its failure state could be recorded",
            ));
        }
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    /// Return a persisted state summary for host recovery tooling. It contains
    /// no prompt bodies, credentials, or provider response text.
    pub async fn session_status(
        &self,
        actor: &str,
        session: &str,
    ) -> Result<Value, DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let row = sqlx::query("SELECT s.chain_id,s.active_fingerprint,t.state,t.phase,t.error_code FROM dsh_bridge_sessions s LEFT JOIN dsh_bridge_turns t ON t.actor=s.actor AND t.session_key=s.session_key AND t.fingerprint=s.active_fingerprint WHERE s.actor=? AND s.session_key=?")
            .bind(actor).bind(session).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let Some(row) = row else {
            return Err(DshBridgeError {
                status: StatusCode::NOT_FOUND,
                code: "session_not_found",
                message: "DSH session not found",
            });
        };
        let active_fingerprint = row
            .try_get::<Option<String>, _>("active_fingerprint")
            .map_err(|_| DshBridgeError::internal())?;
        let retired: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM dsh_bridge_retirements WHERE actor=? AND session_key=?",
        )
        .bind(actor)
        .bind(session)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        let status = json!({"chain_id":row.try_get::<Option<String>,_>("chain_id").map_err(|_|DshBridgeError::internal())?,
            "active":active_fingerprint.is_some(),"active_fingerprint":active_fingerprint,"retired":retired.is_some(),
            "state":row.try_get::<Option<String>,_>("state").map_err(|_|DshBridgeError::internal())?,
            "phase":row.try_get::<Option<String>,_>("phase").map_err(|_|DshBridgeError::internal())?,
            "error_code":row.try_get::<Option<String>,_>("error_code").map_err(|_|DshBridgeError::internal())?});
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(status)
    }

    /// Seal an unresolved DSH session without replaying its provider calls or
    /// external tasks. A fresh session may bind the same Agent only if the
    /// ordinary idle/lease/task checks still pass.
    pub async fn retire_unresolved_session(
        &self,
        actor: &str,
        session: &str,
        fingerprint: &str,
        reason: &str,
    ) -> Result<Value, DshBridgeError> {
        if actor.trim().is_empty()
            || session.trim().is_empty()
            || session.len() > 256
            || fingerprint.len() != 64
            || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
            || reason.trim().is_empty()
            || reason.len() > 512
        {
            return Err(DshBridgeError::invalid(
                "bounded session, fingerprint and reason are required",
            ));
        }
        let _permit = self.lock_session(actor, session).await?;
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        if let Some(row)=sqlx::query("SELECT chain_id,fingerprint,original_state,original_phase,reason FROM dsh_bridge_retirements WHERE actor=? AND session_key=?")
            .bind(actor).bind(session).fetch_optional(&mut *tx).await.map_err(|_|DshBridgeError::internal())? {
            let saved:String=row.try_get("fingerprint").map_err(|_|DshBridgeError::internal())?;
            let saved_reason:String=row.try_get("reason").map_err(|_|DshBridgeError::internal())?;
            if saved!=fingerprint || saved_reason!=reason {return Err(DshBridgeError::conflict("retirement_conflict","Session retirement request differs from its recorded receipt"));}
            let chain:Option<String>=row.try_get("chain_id").map_err(|_|DshBridgeError::internal())?;
            if let Some(id)=chain.as_deref() {
                storage::get_chain(&mut tx,id,actor).await.map_err(|error|match error.code {
                    "forbidden"|"not_found"=>DshBridgeError {status:StatusCode::FORBIDDEN,code:"chain_denied",message:"Agent is unavailable to this host actor"},
                    _=>DshBridgeError::internal()})?;
            }
            let state:String=row.try_get("original_state").map_err(|_|DshBridgeError::internal())?;
            let phase:String=row.try_get("original_phase").map_err(|_|DshBridgeError::internal())?;
            tx.commit().await.map_err(|_|DshBridgeError::internal())?;
            return Ok(json!({"chain_id":chain,"fingerprint":fingerprint,"original_state":state,"original_phase":phase,"retired":true,"replayed":true,"external_work_replayed":false}));
        }
        let row=sqlx::query("SELECT s.chain_id,s.active_fingerprint,t.state,t.phase FROM dsh_bridge_sessions s LEFT JOIN dsh_bridge_turns t ON t.actor=s.actor AND t.session_key=s.session_key AND t.fingerprint=s.active_fingerprint WHERE s.actor=? AND s.session_key=?")
            .bind(actor).bind(session).fetch_optional(&mut *tx).await.map_err(|_|DshBridgeError::internal())?
            .ok_or(DshBridgeError {status:StatusCode::NOT_FOUND,code:"session_not_found",message:"DSH session not found"})?;
        let active: Option<String> = row
            .try_get("active_fingerprint")
            .map_err(|_| DshBridgeError::internal())?;
        let state: Option<String> = row
            .try_get("state")
            .map_err(|_| DshBridgeError::internal())?;
        let phase: Option<String> = row
            .try_get("phase")
            .map_err(|_| DshBridgeError::internal())?;
        if active.as_deref() != Some(fingerprint)
            || !matches!(state.as_deref(), Some("failed" | "indeterminate"))
            || phase.is_none()
        {
            return Err(DshBridgeError::conflict(
                "retirement_state_changed",
                "Only the matching failed or indeterminate turn can be retired",
            ));
        }
        let chain: Option<String> = row
            .try_get("chain_id")
            .map_err(|_| DshBridgeError::internal())?;
        if let Some(id) = chain.as_deref() {
            storage::get_chain(&mut tx, id, actor)
                .await
                .map_err(|error| match error.code {
                    "forbidden" | "not_found" => DshBridgeError {
                        status: StatusCode::FORBIDDEN,
                        code: "chain_denied",
                        message: "Agent is unavailable to this host actor",
                    },
                    _ => DshBridgeError::internal(),
                })?;
        }
        sqlx::query("INSERT INTO dsh_bridge_retirements(actor,session_key,chain_id,fingerprint,original_state,original_phase,reason,created) VALUES(?,?,?,?,?,?,?,?)")
            .bind(actor).bind(session).bind(&chain).bind(fingerprint).bind(state.as_deref()).bind(phase.as_deref()).bind(reason).bind(now() as i64)
            .execute(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(
            json!({"chain_id":chain,"fingerprint":fingerprint,"original_state":state,"original_phase":phase,"retired":true,"replayed":false,"external_work_replayed":false}),
        )
    }

    /// Bind a new DSH session to an already owned, idle Rust Agent. This is a
    /// host operation, never a model tool. It does not replay the old harness
    /// history or resume unresolved external work.
    pub async fn bind_existing_session(
        &self,
        actor: &str,
        session: &str,
        chain_id: &str,
    ) -> Result<Value, DshBridgeError> {
        if actor.trim().is_empty()
            || session.trim().is_empty()
            || session.len() > 256
            || HeaderValue::from_str(session).is_err()
            || chain_id.trim().is_empty()
            || chain_id.len() > 256
        {
            return Err(DshBridgeError::invalid(
                "bounded actor, session and chain identities are required",
            ));
        }
        let _permit = self.lock_session(actor, session).await?;
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let agent =
            storage::get_chain(&mut tx, chain_id, actor)
                .await
                .map_err(|error| match error.code {
                    "forbidden" | "not_found" => DshBridgeError {
                        status: StatusCode::FORBIDDEN,
                        code: "chain_denied",
                        message: "Agent is unavailable to this host actor",
                    },
                    _ => DshBridgeError::internal(),
                })?;
        if let Some(row) =
            sqlx::query("SELECT chain_id FROM dsh_bridge_sessions WHERE actor=? AND session_key=?")
                .bind(actor)
                .bind(session)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| DshBridgeError::internal())?
        {
            let retired: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM dsh_bridge_retirements WHERE actor=? AND session_key=?",
            )
            .bind(actor)
            .bind(session)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| DshBridgeError::internal())?;
            if retired.is_some() {
                return Err(DshBridgeError::conflict(
                    "session_retired",
                    "A retired DSH session cannot be rebound",
                ));
            }
            let bound: Option<String> = row
                .try_get("chain_id")
                .map_err(|_| DshBridgeError::internal())?;
            if bound.as_deref() != Some(chain_id) {
                return Err(DshBridgeError::conflict(
                    "session_already_bound",
                    "DSH session already belongs to another Agent",
                ));
            }
            tx.commit().await.map_err(|_| DshBridgeError::internal())?;
            return Ok(
                json!({"session_id":session,"chain_id":chain_id,"replayed":true,"idle_revalidated":false}),
            );
        }
        let cut = agent["cut_seq"]
            .as_u64()
            .ok_or_else(DshBridgeError::internal)?;
        let idle_contexts = agent["contexts"].as_object().is_some_and(|contexts| {
            contexts.values().all(|context| {
                context["levels"].as_object().is_some_and(|levels| {
                    levels
                        .values()
                        .all(|level| level["pending"].as_array().is_some_and(Vec::is_empty))
                })
            })
        });
        let idle_jobs = agent["jobs"].as_object().is_some_and(|jobs| {
            jobs.values().all(|job| {
                job["base_cut"].as_u64() != Some(cut)
                    || !matches!(
                        job["status"].as_str(),
                        Some("open" | "ready" | "inflight" | "failed")
                    )
            })
        });
        if agent["run_state"].as_str() != Some("running")
            || !agent["worker_claim"].is_null()
            || !idle_contexts
            || !idle_jobs
        {
            return Err(DshBridgeError::conflict(
                "chain_not_idle",
                "Agent must be running with no pending WB, Editor, or Worker",
            ));
        }
        let bound_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM dsh_bridge_sessions s WHERE chain_id=? AND NOT EXISTS (SELECT 1 FROM dsh_bridge_retirements r WHERE r.actor=s.actor AND r.session_key=s.session_key)")
                .bind(chain_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| DshBridgeError::internal())?;
        if bound_count != 0 {
            return Err(DshBridgeError::conflict(
                "chain_already_bound",
                "Agent is already attached to a DSH session",
            ));
        }
        let issued:i64=sqlx::query_scalar("SELECT COUNT(*) FROM dsh_bridge_tool_tasks t JOIN dsh_bridge_sessions s ON s.actor=t.actor AND s.session_key=t.session_key WHERE s.chain_id=? AND t.task_state='issued'")
            .bind(chain_id).fetch_one(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        if issued != 0 {
            return Err(DshBridgeError::conflict(
                "chain_not_idle",
                "Agent has unresolved DSH tool mappings",
            ));
        }
        let tasks = sqlx::query("SELECT state FROM host_tool_tasks")
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| DshBridgeError::internal())?;
        for row in tasks {
            let text: String = row
                .try_get("state")
                .map_err(|_| DshBridgeError::internal())?;
            let task: Value =
                serde_json::from_str(&text).map_err(|_| DshBridgeError::internal())?;
            if task["chain_id"].as_str() == Some(chain_id)
                && !matches!(
                    task["status"].as_str(),
                    Some("completed" | "delivered_unattested")
                )
            {
                return Err(DshBridgeError::conflict(
                    "chain_not_idle",
                    "Agent has unresolved host tool work",
                ));
            }
        }
        let has_tools: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='sic_dynamic_toolsets'",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        if has_tools.is_some() {
            let row = sqlx::query(
                "SELECT state,digest FROM sic_dynamic_toolsets WHERE chain_id=? AND actor=?",
            )
            .bind(chain_id)
            .bind(actor)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| DshBridgeError::internal())?;
            if let Some(row) = row {
                let text: String = row
                    .try_get("state")
                    .map_err(|_| DshBridgeError::internal())?;
                let digest: String = row
                    .try_get("digest")
                    .map_err(|_| DshBridgeError::internal())?;
                if storage::sha256(text.as_bytes()) != digest {
                    return Err(DshBridgeError::internal());
                }
                let state: Value =
                    serde_json::from_str(&text).map_err(|_| DshBridgeError::internal())?;
                let idle = state["tools"].as_object().is_some_and(|tools| {
                    tools
                        .values()
                        .all(|tool| tool["in_flight"].as_array().is_some_and(Vec::is_empty))
                });
                if !idle {
                    return Err(DshBridgeError::conflict(
                        "chain_not_idle",
                        "Agent has unresolved tool leases",
                    ));
                }
            }
        }
        // Empty fingerprint forces tools.sync on the first request, including
        // an empty catalog, so host grants reflect the actual DSH table.
        sqlx::query("INSERT INTO dsh_bridge_sessions(actor,session_key,model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count) VALUES(?,?,?,?,'[]','[]',NULL,'',0,0)")
            .bind(actor).bind(session).bind(&self.config.model_alias).bind(chain_id)
            .execute(&mut *tx).await.map_err(|_|DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(
            json!({"session_id":session,"chain_id":chain_id,"replayed":false,"idle_revalidated":true}),
        )
    }
}

fn history_delta(
    history: &[Value],
    messages: &[Value],
    pending_tasks: &[PendingTask],
) -> Result<Vec<Value>, DshBridgeError> {
    if messages.len() <= history.len() {
        return Err(DshBridgeError::conflict(
            "history_mismatch",
            "DSH history was truncated, changed, or contains no new input",
        ));
    }
    if history.iter().zip(messages).any(|(expected, actual)| {
        expected != actual && !generated_assistant_equivalent(expected, actual)
    }) {
        return Err(DshBridgeError::conflict(
            "history_mismatch",
            "DSH history prefix changed",
        ));
    }
    let additions = messages[history.len()..].to_vec();
    if !pending_tasks.is_empty() {
        if additions.len() != pending_tasks.len() {
            return Err(DshBridgeError::conflict(
                "tool_results_incomplete",
                "DSH must return exactly one result for every issued host task",
            ));
        }
        let mut received = std::collections::HashSet::new();
        for message in &additions {
            if message["role"].as_str() != Some("tool") {
                return Err(DshBridgeError::invalid(
                    "only tool results are accepted while host tasks are pending",
                ));
            }
            let call_id = message["tool_call_id"]
                .as_str()
                .ok_or_else(|| DshBridgeError::invalid("tool result call ID is required"))?;
            let task = pending_tasks
                .iter()
                .find(|task| task.call_id == call_id)
                .ok_or_else(|| {
                    DshBridgeError::conflict(
                        "tool_result_unmatched",
                        "DSH tool result does not match an issued host task",
                    )
                })?;
            if !received.insert(call_id.to_owned()) {
                return Err(DshBridgeError::conflict(
                    "tool_result_duplicate",
                    "duplicate DSH tool result",
                ));
            }
            if message
                .get("name")
                .is_some_and(|name| name.as_str() != Some(task.tool_name.as_str()))
            {
                return Err(DshBridgeError::conflict(
                    "tool_result_name_mismatch",
                    "DSH tool result name does not match the issued task",
                ));
            }
        }
        if received.len() != pending_tasks.len() {
            return Err(DshBridgeError::conflict(
                "tool_results_incomplete",
                "DSH did not return every issued tool result",
            ));
        }
        return Ok(additions);
    }
    if history.is_empty() {
        let mut saw_user = false;
        for message in &additions {
            match message["role"].as_str() {
                Some("system" | "developer") if !saw_user => {}
                Some("user") => saw_user = true,
                _ => {
                    return Err(DshBridgeError::invalid(
                        "a new DSH session must begin with protected instructions and user text only",
                    ));
                }
            }
        }
        if !saw_user {
            return Err(DshBridgeError::invalid("DSH session requires user input"));
        }
    } else if additions
        .iter()
        .any(|message| message["role"].as_str() != Some("user"))
    {
        return Err(DshBridgeError::invalid(
            "each DSH turn may add user text only",
        ));
    }
    Ok(additions)
}

fn generated_assistant_equivalent(expected: &Value, actual: &Value) -> bool {
    if expected.get("role").and_then(Value::as_str) != Some("assistant")
        || actual.get("role").and_then(Value::as_str) != Some("assistant")
    {
        return false;
    }
    let expected_content = expected.get("content").filter(|value| !value.is_null());
    let actual_content = actual.get("content").filter(|value| !value.is_null());
    if expected_content != actual_content {
        return false;
    }
    let (Some(expected_calls), Some(actual_calls)) = (
        expected.get("tool_calls").and_then(Value::as_array),
        actual.get("tool_calls").and_then(Value::as_array),
    ) else {
        return expected.get("tool_calls").is_none() && actual.get("tool_calls").is_none();
    };
    expected_calls.len() == actual_calls.len()
        && expected_calls
            .iter()
            .zip(actual_calls)
            .all(|(left, right)| {
                left.get("id") == right.get("id")
                    && left.get("type") == right.get("type")
                    && left.pointer("/function/name") == right.pointer("/function/name")
                    && parse_tool_arguments(left) == parse_tool_arguments(right)
            })
}

fn parse_tool_arguments(call: &Value) -> Option<Value> {
    serde_json::from_str(call.pointer("/function/arguments")?.as_str()?).ok()
}

fn validate_worker_round(
    calls: &[Value],
    tool_results: &[Value],
    host_tasks: &[Value],
) -> Result<(), String> {
    let mut call_names = std::collections::HashMap::new();
    for call in calls {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Worker returned a tool call without an ID".to_owned())?;
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .ok_or_else(|| "Worker returned a tool call without a name".to_owned())?;
        if call_names.insert(id.to_owned(), name.to_owned()).is_some() {
            return Err("Worker returned duplicate tool call IDs".into());
        }
    }
    let mut closed = std::collections::HashSet::new();
    for result in tool_results {
        let id = result
            .get("tool_call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Worker tool result has no call ID".to_owned())?;
        if !call_names.contains_key(id) || !closed.insert(id.to_owned()) {
            return Err("Worker returned an unmatched or duplicate internal tool result".into());
        }
    }
    for task in host_tasks {
        let id = task
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "host task has no call ID".to_owned())?;
        if call_names.get(id).map(String::as_str) != Some("sic_tool_call")
            || !closed.insert(id.to_owned())
        {
            return Err("host task does not match a unique sic_tool_call".into());
        }
    }
    if closed.len() != call_names.len() {
        return Err("Worker returned tool calls without internal results or host tasks".into());
    }
    Ok(())
}

fn validate_host_tasks(
    actor: &str,
    chain_id: &str,
    tasks: &[Value],
    calls: &[Value],
    harness_tools: &[Value],
) -> Result<Vec<PendingTask>, String> {
    let mut result = Vec::with_capacity(tasks.len());
    let mut call_ids = std::collections::HashSet::new();
    let mut task_ids = std::collections::HashSet::new();
    for task in tasks {
        if task.get("chain_id").and_then(Value::as_str) != Some(chain_id)
            || task.get("actor").and_then(Value::as_str) != Some(actor)
            || task.get("status").and_then(Value::as_str) != Some("pending")
        {
            return Err("host task is outside the authenticated DSH scope".into());
        }
        let task_id = task
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "host task ID is missing".to_owned())?;
        let call_id = task
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "host task call ID is missing".to_owned())?;
        if !call_ids.insert(call_id.to_owned()) || !task_ids.insert(task_id.to_owned()) {
            return Err("host task IDs must be unique".into());
        }
        let descriptor = task
            .get("descriptor")
            .ok_or_else(|| "host task descriptor is missing".to_owned())?;
        let tool_name = descriptor
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "host task tool name is missing".to_owned())?;
        if !harness_tools
            .iter()
            .any(|candidate| candidate == descriptor)
        {
            return Err(
                "host task descriptor differs from the authenticated DSH tool table".into(),
            );
        }
        let arguments = task
            .get("arguments")
            .filter(|arguments| arguments.is_object())
            .ok_or_else(|| "host task arguments must be an object".to_owned())?
            .clone();
        let call = calls
            .iter()
            .find(|call| call.get("id").and_then(Value::as_str) == Some(call_id))
            .ok_or_else(|| "host task call is absent from Worker response".to_owned())?;
        let input: Value = serde_json::from_str(
            call.pointer("/function/arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| "sic_tool_call arguments are missing".to_owned())?,
        )
        .map_err(|_| "sic_tool_call arguments are invalid JSON".to_owned())?;
        if input.get("name").and_then(Value::as_str) != Some(tool_name)
            || input.get("arguments") != Some(&arguments)
        {
            return Err("host task does not preserve sic_tool_call name and arguments".into());
        }
        result.push(PendingTask {
            task_id: task_id.to_owned(),
            call_id: call_id.to_owned(),
            tool_name: tool_name.to_owned(),
            arguments,
        });
    }
    Ok(result)
}

fn chat_reply(request: &wire::DshChatRequest, message: Value, finish_reason: &str) -> Value {
    json!({
        "id":format!("sic-{}",Uuid::new_v4().simple()),"created":now(),
        "model":request.model_alias,"choices":[{"index":0,"finish_reason":finish_reason,"message":message}]
    })
}

fn encode_reply(reply: &Value, include_usage: bool) -> Result<String, (DshBridgeError, bool)> {
    let bytes = wire::buffered_sse(reply, include_usage)
        .map_err(|_| (DshBridgeError::core_failure(), true))?;
    String::from_utf8(bytes).map_err(|_| (DshBridgeError::core_failure(), true))
}

#[derive(Clone)]
struct RouteState {
    bridge: Arc<DshBridge>,
    resolve_actor: ActorResolver,
}

/// A router intended to be nested under `/v1`. The provided resolver must
/// authenticate the request and return its host-authorized actor identity.
pub fn router(bridge: Arc<DshBridge>, resolve_actor: ActorResolver) -> Router {
    let state = RouteState {
        bridge,
        resolve_actor,
    };
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/sic/status", post(session_status_route))
        .route("/sic/bind", post(bind_existing_session_route))
        .route("/sic/retire", post(retire_unresolved_session_route))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(no_store))
}

async fn retire_unresolved_session_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| fields.len() == 3) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id, fingerprint and reason are required",
        );
    };
    let (Some(session), Some(fingerprint), Some(reason)) = (
        fields.get("session_id").and_then(Value::as_str),
        fields.get("fingerprint").and_then(Value::as_str),
        fields.get("reason").and_then(Value::as_str),
    ) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id, fingerprint and reason are required",
        );
    };
    match state
        .bridge
        .retire_unresolved_session(&actor, session, fingerprint, reason)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

async fn bind_existing_session_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| fields.len() == 2) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id and chain_id are required",
        );
    };
    let (Some(session), Some(chain)) = (
        fields.get("session_id").and_then(Value::as_str),
        fields.get("chain_id").and_then(Value::as_str),
    ) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id and chain_id are required",
        );
    };
    match state
        .bridge
        .bind_existing_session(&actor, session, chain)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

async fn session_status_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let session = match value
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 256)
    {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "A bounded session_id is required",
            );
        }
    };
    match state.bridge.session_status(&actor, session).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

async fn chat_completions(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    match state.bridge.handle(&actor, &headers, &value).await {
        Ok(bytes) => {
            let mut response = Response::new(bytes.into());
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream; charset=utf-8"),
            );
            response
        }
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

async fn no_store(request: axum::http::Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error":{"type":"sic_dsh_bridge_error","code":code,"message":message}})),
    )
        .into_response()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stable_tool_claim_id(
    actor: &str,
    session: &str,
    task_id: &str,
) -> Result<String, DshBridgeError> {
    let digest = storage::digest(&json!({"actor":actor,"session":session,"task_id":task_id}))
        .map_err(|_| DshBridgeError::internal())?;
    Ok(format!("dsh:{digest}"))
}
