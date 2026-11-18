//! Host-authenticated scheduling for model-requested dynamic tool calls.
//!
//! This module accepts native host tool names and the compatibility
//! `sic_tool_*` names and arguments, then
//! delegates authorization and lease state to `ToolStore`. It does not invoke
//! DSH or validate full JSON Schema arguments; it returns a lease to a trusted
//! host runner and persists that runner's result before releasing the lease.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    core::{Core, OperationContext},
    error::{CoreError, Result},
    tool_store::ToolStore,
    toolset::{ExecutionLease, ToolDescriptor, ToolUpdate},
};

/// A host-issued capability for native tools operating on one candidate.
/// It is deliberately not deserializable from model tool arguments. Worker
/// execution scopes cannot be used as Editor capabilities.
#[derive(Clone, Debug)]
pub struct AuthenticatedEditorScope {
    actor: String,
    chain_id: Option<String>,
    source_id: String,
    source_digest: String,
    version: u64,
    unit_ids: Vec<String>,
    native_tools: Vec<String>,
}

impl AuthenticatedEditorScope {
    pub fn from_authenticated_host(
        context: &OperationContext,
        source_id: impl Into<String>,
        source_digest: impl Into<String>,
        version: u64,
        unit_ids: Vec<String>,
        native_tools: Vec<String>,
    ) -> Result<Self> {
        let source_id = source_id.into();
        let source_digest = source_digest.into();
        if !context.allow_write || context.actor.trim().is_empty()
            || source_id.trim().is_empty() || source_digest.trim().is_empty()
            || unit_ids.is_empty() || native_tools.is_empty()
            || unit_ids.iter().chain(&native_tools).any(|id| id.trim().is_empty())
        {
            return Err(CoreError::forbidden("host candidate editing authorization required"));
        }
        Ok(Self { actor: context.actor.clone(), chain_id: context.chain_id.clone(),
            source_id, source_digest, version, unit_ids, native_tools })
    }
}

/// Execute the edit purpose of a native invocation without renaming its tool
/// or changing its parameters. The trusted Harness mapper interprets the
/// original invocation against its candidate workspace; it must not execute
/// task side effects. Its result is restricted to the granted text anchors
/// before the atomic, persistent native.commit operation sees it.
pub async fn dispatch_editor_native_tool<F>(
    core: &Core,
    context: &OperationContext,
    scope: &AuthenticatedEditorScope,
    tool_name: &str,
    arguments: &Value,
    map_candidate: F,
) -> Result<Value>
where F: FnOnce(&str, &Value) -> Result<Vec<crate::transparent::TextEdit>>,
{
    if !context.allow_write || context.actor != scope.actor || context.chain_id != scope.chain_id
        || !scope.native_tools.iter().any(|name| name == tool_name)
    {
        return Err(CoreError::forbidden("native tool is outside the host Editor scope"));
    }
    let edits = map_candidate(tool_name, arguments)?;
    if edits.iter().any(|edit| !scope.unit_ids.contains(&edit.unit_id)) {
        return Err(CoreError::forbidden("native edit targets an unassigned candidate unit"));
    }
    let edits: Vec<Value> = edits.iter().map(|edit| serde_json::json!({
        "unit_id":edit.unit_id,"expected_text":edit.expected_text,"replacement":edit.replacement,
    })).collect();
    core.operate("native.commit", context, &serde_json::json!({
        "source_id":scope.source_id,"expected_source_digest":scope.source_digest,
        "expected_version":scope.version,"edits":edits,
        "native_tool":{"name":tool_name,"arguments_sha256":crate::storage::digest(arguments)?},
    })).await
}

const RESULTS_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_tool_dispatch_results (
    lease_id TEXT NOT NULL,
    chain_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    succeeded INTEGER NOT NULL,
    result_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(chain_id, lease_id)
)";
const ROUNDS_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_tool_worker_rounds (
    chain_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    response_id TEXT NOT NULL,
    evicted_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(chain_id, actor, response_id)
)";

/// Constructed only from a trusted host invocation. The model request supplies
/// neither identity, scope, nor this model-call grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedWorkerScope {
    actor: String,
    chain_id: String,
    allow_model_tool_calls: bool,
}

impl AuthenticatedWorkerScope {
    pub fn from_authenticated_host(
        actor: impl Into<String>,
        chain_id: impl Into<String>,
        allow_model_tool_calls: bool,
    ) -> Result<Self> {
        let actor = actor.into();
        let chain_id = chain_id.into();
        if actor.trim().is_empty() || chain_id.trim().is_empty() {
            return Err(CoreError::invalid(
                "authenticated Worker actor and chain are required",
            ));
        }
        Ok(Self {
            actor,
            chain_id,
            allow_model_tool_calls,
        })
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolDispatch {
    Updated {
        update: ToolUpdate,
    },
    /// The host must execute this descriptor/argument pair and persist the
    /// outcome with `persist_tool_result`; the model never executes it itself.
    HostExecutionRequired {
        lease: ExecutionLease,
        descriptor: ToolDescriptor,
        arguments: Value,
        evicted: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PersistedToolResult {
    pub lease_id: String,
    pub tool_name: String,
    /// None means the host delivered a result but did not attest success.
    pub succeeded: Option<bool>,
    pub result: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRoundCompletion {
    pub advanced: bool,
    pub evicted: Vec<String>,
}

/// Dispatch one model tool call inside the caller's SQLite transaction.
/// `tool_name` is the advertised native name or compatibility SIC name;
/// `arguments` are untrusted model
/// data and can never modify ACL, identity, chain, schema, or lease state.
pub async fn dispatch_model_tool_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &AuthenticatedWorkerScope,
    tool_name: &str,
    arguments: &Value,
) -> Result<ToolDispatch> {
    require_model_calls(scope)?;
    let obj = arguments
        .as_object()
        .ok_or_else(|| CoreError::invalid("tool arguments must be an object"))?;
    match tool_name {
        "sic_tool_update" => {
            exact_keys_optional(obj, &["select", "fold"])?;
            let select = string_array(obj, "select")?;
            let fold = string_array(obj, "fold")?;
            // The only real bound on select is the dynamic schema window; fold
            // never consumes a schema slot. Exceeding the window is not an error:
            // the host folds least-recently-used tools and names them in the
            // result, which is the documented capacity path.
            let capacity = crate::tool_store::dynamic_schema_capacity()?;
            if select.len() > capacity {
                return Err(CoreError::new(
                    "tool_window_exceeded",
                    format!(
                        "select has {} names but the dynamic schema window holds at most {capacity}; split the request or rely on folding",
                        select.len()
                    ),
                    422,
                ));
            }
            if select.is_empty() && fold.is_empty() {
                return Err(CoreError::invalid("tool update requires select or fold"));
            }
            let update =
                ToolStore::update_in_transaction(tx, &scope.chain_id, &scope.actor, &select, &fold)
                    .await?;
            Ok(ToolDispatch::Updated { update })
        }
        "sic_tool_call" => {
            exact_keys(obj, &["name", "arguments"])?;
            let name = required_string(arguments, "name")?;
            let tool_arguments = arguments
                .get("arguments")
                .filter(|v| v.is_object())
                .ok_or_else(|| CoreError::invalid("sic_tool_call.arguments must be an object"))?;
            let started =
                ToolStore::begin_execution_in_transaction(tx, &scope.chain_id, &scope.actor, name)
                    .await?;
            Ok(ToolDispatch::HostExecutionRequired {
                lease: started.lease,
                descriptor: started.descriptor,
                arguments: tool_arguments.clone(),
                evicted: started.evicted,
            })
        }
        _ => {
            let started = ToolStore::begin_execution_in_transaction(
                tx, &scope.chain_id, &scope.actor, tool_name,
            ).await?;
            Ok(ToolDispatch::HostExecutionRequired {
                lease: started.lease,
                descriptor: started.descriptor,
                arguments: arguments.clone(),
                evicted: started.evicted,
            })
        }
    }
}

/// Persist an outcome from the trusted host runner and release its lease in
/// the same transaction. A caller may store either success output or a
/// structured failure value; no model-controlled result can finish a lease.
pub async fn persist_tool_result_in_transaction<S: Into<Option<bool>>>(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &AuthenticatedWorkerScope,
    lease: &ExecutionLease,
    succeeded: S,
    result: &Value,
) -> Result<PersistedToolResult> {
    let succeeded = succeeded.into();
    let active = ToolStore::active_leases_in_transaction(tx, &scope.chain_id, &scope.actor).await?;
    if !active
        .iter()
        .any(|candidate| candidate.id() == lease.id() && candidate.tool_name() == lease.tool_name())
    {
        return Err(CoreError::conflict(
            "tool execution lease is not active in this host scope",
        ));
    }
    sqlx::query(RESULTS_SQL).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO sic_tool_dispatch_results(lease_id,chain_id,actor,tool_name,succeeded,result_json) VALUES(?,?,?,?,?,?)")
        .bind(lease.id())
        .bind(&scope.chain_id)
        .bind(&scope.actor)
        .bind(lease.tool_name())
        .bind(match succeeded {Some(true)=>1_i64,Some(false)=>0_i64,None=>-1_i64})
        .bind(serde_json::to_string(result).map_err(|_| CoreError::invalid("tool result is not valid JSON"))?)
        .execute(&mut **tx).await?;
    ToolStore::finish_execution_in_transaction(tx, &scope.chain_id, &scope.actor, lease).await?;
    Ok(PersistedToolResult {
        lease_id: lease.id().to_owned(),
        tool_name: lease.tool_name().to_owned(),
        succeeded,
        result: result.clone(),
    })
}

/// Read a previously committed host outcome for crash recovery or model
/// continuation. The actor and chain are taken only from host scope.
pub async fn load_tool_result_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &AuthenticatedWorkerScope,
    lease_id: &str,
) -> Result<Option<PersistedToolResult>> {
    sqlx::query(RESULTS_SQL).execute(&mut **tx).await?;
    let row = sqlx::query("SELECT tool_name,succeeded,result_json FROM sic_tool_dispatch_results WHERE lease_id=? AND chain_id=? AND actor=?")
        .bind(lease_id).bind(&scope.chain_id).bind(&scope.actor).fetch_optional(&mut **tx).await?;
    row.map(|row| {
        let json_text: String = row.try_get("result_json")?;
        let result = serde_json::from_str(&json_text).map_err(|_| {
            CoreError::new("tool_result_invalid", "stored tool result is invalid", 500)
        })?;
        Ok(PersistedToolResult {
            lease_id: lease_id.to_owned(),
            tool_name: row.try_get("tool_name")?,
            succeeded: match row.try_get::<i64, _>("succeeded")? {
                1 => Some(true),
                0 => Some(false),
                -1 => None,
                _ => {
                    return Err(CoreError::new(
                        "tool_result_invalid",
                        "stored tool outcome is invalid",
                        500,
                    ));
                }
            },
            result,
        })
    })
    .transpose()
}

/// Advance exactly once for a host-issued Worker response ID. Invoke only at
/// the trusted Worker response-complete boundary, not per tool call or token.
pub async fn complete_worker_response_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &AuthenticatedWorkerScope,
    host_response_id: &str,
) -> Result<WorkerRoundCompletion> {
    complete_worker_response_with_new_expansions_in_transaction(tx, scope, host_response_id, &[])
        .await
}

pub async fn complete_worker_response_with_new_expansions_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &AuthenticatedWorkerScope,
    host_response_id: &str,
    newly_expanded: &[String],
) -> Result<WorkerRoundCompletion> {
    if host_response_id.trim().is_empty() {
        return Err(CoreError::invalid("host Worker response ID is required"));
    }
    sqlx::query(ROUNDS_SQL).execute(&mut **tx).await?;
    let existing = sqlx::query("SELECT evicted_json FROM sic_tool_worker_rounds WHERE chain_id=? AND actor=? AND response_id=?")
        .bind(&scope.chain_id).bind(&scope.actor).bind(host_response_id)
        .fetch_optional(&mut **tx).await?;
    if let Some(row) = existing {
        let encoded: String = row.try_get("evicted_json")?;
        let evicted = serde_json::from_str(&encoded).map_err(|_| {
            CoreError::new(
                "tool_round_invalid",
                "stored Worker round receipt is invalid",
                500,
            )
        })?;
        return Ok(WorkerRoundCompletion {
            advanced: false,
            evicted,
        });
    }
    let evicted = ToolStore::advance_worker_round_with_new_expansions_in_transaction(
        tx,
        &scope.chain_id,
        &scope.actor,
        newly_expanded,
    )
    .await?;
    let encoded = serde_json::to_string(&evicted)
        .map_err(|_| CoreError::invalid("eviction list is invalid"))?;
    sqlx::query("INSERT INTO sic_tool_worker_rounds(chain_id,actor,response_id,evicted_json) VALUES(?,?,?,?)")
        .bind(&scope.chain_id).bind(&scope.actor).bind(host_response_id).bind(encoded)
        .execute(&mut **tx).await?;
    Ok(WorkerRoundCompletion {
        advanced: true,
        evicted,
    })
}

fn require_model_calls(scope: &AuthenticatedWorkerScope) -> Result<()> {
    if !scope.allow_model_tool_calls {
        return Err(CoreError::new(
            "model_denied",
            "host has not authorized model tool calls",
            403,
        ));
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| CoreError::invalid(format!("{key} must be a non-empty string")))
}

fn exact_keys(obj: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if obj.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(CoreError::invalid(
            "tool arguments contain an unsupported field",
        ));
    }
    if allowed.iter().any(|key| !obj.contains_key(*key)) {
        return Err(CoreError::invalid(
            "tool arguments are missing a required field",
        ));
    }
    Ok(())
}

fn exact_keys_optional(obj: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if obj.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(CoreError::invalid(
            "tool arguments contain an unsupported field",
        ));
    }
    Ok(())
}

/// Read an optional string array. Absent means empty; a wrong shape, empty
/// entry, or duplicate name is rejected so the model gets a precise error.
fn string_array(obj: &serde_json::Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let Some(value) = obj.get(key) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| CoreError::invalid(format!("{key} must be an array of tool names")))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let name = item
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                CoreError::invalid(format!("{key} entries must be non-empty strings"))
            })?;
        if out.iter().any(|existing: &String| existing == name) {
            return Err(CoreError::invalid(format!(
                "{key} contains a duplicate tool name"
            )));
        }
        out.push(name.to_owned());
    }
    Ok(out)
}

