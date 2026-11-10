//! Transactional SQLite persistence for the dynamic Rust toolset.
//!
//! The service methods ending in `_in_transaction` can participate in a host
//! or Core transaction. Convenience methods open/commit one Store transaction
//! per action. This is a persistence/authorization layer; it does not execute
//! DSH tools or model calls.

use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    error::{CoreError, Result},
    storage::Store,
    toolset::{
        ExecutionLease, ExecutionStart, Expansion, ToolAcl, ToolDescriptor, ToolSet, ToolSummary,
    },
};

const TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_dynamic_toolsets (
    chain_id TEXT PRIMARY KEY NOT NULL,
    actor TEXT NOT NULL,
    state TEXT NOT NULL,
    digest TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
)";

const MAX_PROVIDER_TOOL_SCHEMAS: usize = 128;

pub fn dynamic_schema_capacity() -> Result<usize> {
    let fixed = crate::prefix::common_tool_registry().len();
    MAX_PROVIDER_TOOL_SCHEMAS.checked_sub(fixed).ok_or_else(|| {
        CoreError::new(
            "tool_schema_capacity_exceeded",
            "fixed SIC tool schemas exceed the provider schema limit",
            500,
        )
    })
}

fn constrain_capacity(set: &mut ToolSet) -> Result<bool> {
    let limit = dynamic_schema_capacity()?;
    let prior_limit = set.max_expanded();
    let evicted = set.constrain_expanded_limit(limit).map_err(tool_error)?;
    Ok(prior_limit != set.max_expanded() || !evicted.is_empty())
}

#[derive(Clone)]
pub struct ToolStore {
    store: Store,
}

impl ToolStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn initialize(&self) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::initialize_in_transaction(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Create the service table inside a caller-owned transaction.
    pub async fn initialize_in_transaction(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        sqlx::query(TABLE_SQL).execute(&mut **tx).await?;
        Ok(())
    }

    /// Host provisioning: establish actor-bound ACL and initial tool catalog.
    /// This cannot be called from a model-facing operation unless the caller
    /// has independently authenticated a host authorization.
    pub async fn configure_from_host(
        &self,
        chain_id: &str,
        actor: &str,
        acl: ToolAcl,
        tools: Vec<(ToolDescriptor, bool)>,
    ) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::configure_from_host_in_transaction(&mut tx, chain_id, actor, acl, tools).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn configure_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        acl: ToolAcl,
        tools: Vec<(ToolDescriptor, bool)>,
    ) -> Result<()> {
        validate_scope(chain_id, actor)?;
        Self::initialize_in_transaction(tx).await?;
        if load_raw(tx, chain_id).await?.is_some() {
            return Err(CoreError::conflict(
                "toolset is already configured for this chain",
            ));
        }
        let mut set = ToolSet::with_limits(
            actor.to_owned(),
            acl,
            crate::toolset::DEFAULT_IDLE_ROUNDS,
            dynamic_schema_capacity()?,
        );
        for (descriptor, host_fixed) in tools {
            set.register_tool(descriptor, host_fixed)
                .map_err(tool_error)?;
        }
        save_raw(tx, chain_id, actor, &set, false).await
    }

    /// Replace ACL grants from a trusted host update. Active leases and tool
    /// recency remain in the same serialized state; every subsequent request
    /// rechecks the newly stored grants.
    pub async fn update_acl_from_host(
        &self,
        chain_id: &str,
        actor: &str,
        acl: ToolAcl,
    ) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::update_acl_from_host_in_transaction(&mut tx, chain_id, actor, acl).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_acl_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        acl: ToolAcl,
    ) -> Result<()> {
        mutate(tx, chain_id, actor, |set| {
            set.update_acl(acl);
            Ok(())
        })
        .await
    }

    pub async fn sync_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        acl: ToolAcl,
        tools: Vec<(ToolDescriptor, bool)>,
    ) -> Result<()> {
        mutate(tx, chain_id, actor, |set| {
            constrain_capacity(set).map_err(|error| error.message)?;
            set.sync_from_host(acl, tools)?;
            constrain_capacity(set).map_err(|error| error.message)?;
            Ok(())
        })
        .await
    }

    /// Host provisioning for an additional descriptor; the model cannot add
    /// tools or schemas through search, expansion, or execution methods.
    pub async fn register_tool_from_host(
        &self,
        chain_id: &str,
        actor: &str,
        descriptor: ToolDescriptor,
        host_fixed: bool,
    ) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::register_tool_from_host_in_transaction(
            &mut tx, chain_id, actor, descriptor, host_fixed,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn register_tool_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        descriptor: ToolDescriptor,
        host_fixed: bool,
    ) -> Result<()> {
        mutate(tx, chain_id, actor, |set| {
            constrain_capacity(set).map_err(|error| error.message)?;
            set.register_tool(descriptor, host_fixed)?;
            constrain_capacity(set).map_err(|error| error.message)?;
            Ok(())
        })
        .await
    }

    pub async fn search_descriptions(
        &self,
        chain_id: &str,
        actor: &str,
        query: &str,
    ) -> Result<Vec<ToolSummary>> {
        let mut tx = self.store.begin().await?;
        let result =
            Self::search_descriptions_in_transaction(&mut tx, chain_id, actor, query).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn search_descriptions_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        query: &str,
    ) -> Result<Vec<ToolSummary>> {
        let set = load(tx, chain_id, actor).await?;
        Ok(set.search_descriptions(query))
    }

    pub async fn expanded_descriptors_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
    ) -> Result<Vec<ToolDescriptor>> {
        let mut set = load(tx, chain_id, actor).await?;
        if constrain_capacity(&mut set)? {
            save_raw(tx, chain_id, actor, &set, true).await?;
        }
        if set.expanded_count() > dynamic_schema_capacity()? {
            return Err(CoreError::conflict(
                "expanded dynamic schema count exceeds the complete provider schema limit",
            ));
        }
        Ok(set.expanded_descriptors())
    }

    pub async fn request_expansion(
        &self,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<Expansion> {
        let mut tx = self.store.begin().await?;
        let result =
            Self::request_expansion_in_transaction(&mut tx, chain_id, actor, tool_name).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn request_expansion_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<Expansion> {
        mutate(tx, chain_id, actor, |set| {
            constrain_capacity(set).map_err(|error| error.message)?;
            set.request_expansion(tool_name)
        })
        .await
    }

    pub async fn request_reclaim(
        &self,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<bool> {
        let mut tx = self.store.begin().await?;
        let result =
            Self::request_reclaim_in_transaction(&mut tx, chain_id, actor, tool_name).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn request_reclaim_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<bool> {
        mutate(tx, chain_id, actor, |set| set.request_reclaim(tool_name)).await
    }

    pub async fn begin_execution(
        &self,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<ExecutionStart> {
        let mut tx = self.store.begin().await?;
        let result =
            Self::begin_execution_in_transaction(&mut tx, chain_id, actor, tool_name).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn begin_execution_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        tool_name: &str,
    ) -> Result<ExecutionStart> {
        mutate(tx, chain_id, actor, |set| {
            constrain_capacity(set).map_err(|error| error.message)?;
            set.begin_execution(tool_name)
        })
        .await
    }

    pub async fn finish_execution(
        &self,
        chain_id: &str,
        actor: &str,
        lease: &ExecutionLease,
    ) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::finish_execution_in_transaction(&mut tx, chain_id, actor, lease).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn finish_execution_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
        lease: &ExecutionLease,
    ) -> Result<()> {
        mutate(tx, chain_id, actor, |set| set.finish_execution(lease)).await
    }

    /// Called once after a Worker response is complete, inside the host/Core
    /// transaction that commits that response boundary.
    pub async fn advance_worker_round(&self, chain_id: &str, actor: &str) -> Result<Vec<String>> {
        let mut tx = self.store.begin().await?;
        let evicted = Self::advance_worker_round_in_transaction(&mut tx, chain_id, actor).await?;
        tx.commit().await?;
        Ok(evicted)
    }

    pub async fn advance_worker_round_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
    ) -> Result<Vec<String>> {
        mutate(tx, chain_id, actor, ToolSet::advance_worker_round).await
    }

    /// Recover leases after process restart without redispatching them. The
    /// caller must reconcile each external outcome and explicitly finish it.
    pub async fn active_leases(&self, chain_id: &str, actor: &str) -> Result<Vec<ExecutionLease>> {
        let mut tx = self.store.begin().await?;
        let result = Self::active_leases_in_transaction(&mut tx, chain_id, actor).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn active_leases_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        chain_id: &str,
        actor: &str,
    ) -> Result<Vec<ExecutionLease>> {
        let set = load(tx, chain_id, actor).await?;
        let serialized = set.to_json().map_err(tool_error)?;
        let value: Value = serde_json::from_str(&serialized)
            .map_err(|_| CoreError::new("tool_state_invalid", "tool state is invalid", 500))?;
        let mut leases = Vec::new();
        if let Some(tools) = value.get("tools").and_then(Value::as_object) {
            for (tool_name, state) in tools {
                if let Some(ids) = state.get("in_flight").and_then(Value::as_array) {
                    for id in ids.iter().filter_map(Value::as_str) {
                        let lease = serde_json::from_value(json!({
                            "id":id,
                            "tool_name":tool_name
                        }))
                        .map_err(|_| {
                            CoreError::new("tool_state_invalid", "tool lease state is invalid", 500)
                        })?;
                        leases.push(lease);
                    }
                }
            }
        }
        Ok(leases)
    }
}

async fn mutate<T, F>(
    tx: &mut Transaction<'_, Sqlite>,
    chain_id: &str,
    actor: &str,
    mutation: F,
) -> Result<T>
where
    F: FnOnce(&mut ToolSet) -> std::result::Result<T, String>,
{
    let mut set = load(tx, chain_id, actor).await?;
    let output = mutation(&mut set).map_err(tool_error)?;
    save_raw(tx, chain_id, actor, &set, true).await?;
    Ok(output)
}

async fn load(tx: &mut Transaction<'_, Sqlite>, chain_id: &str, actor: &str) -> Result<ToolSet> {
    validate_scope(chain_id, actor)?;
    ToolStore::initialize_in_transaction(tx).await?;
    let row = load_raw(tx, chain_id).await?;
    let Some((stored_actor, state, digest)) = row else {
        return Err(CoreError::new(
            "not_found",
            "toolset is not configured",
            404,
        ));
    };
    if stored_actor != actor {
        return Err(CoreError::forbidden("toolset actor mismatch"));
    }
    if crate::storage::sha256(state.as_bytes()) != digest {
        return Err(CoreError::new(
            "integrity_error",
            "stored toolset integrity check failed",
            500,
        ));
    }
    ToolSet::from_json(&state)
        .map_err(|_| CoreError::new("tool_state_invalid", "stored toolset state is invalid", 500))
}

async fn load_raw(
    tx: &mut Transaction<'_, Sqlite>,
    chain_id: &str,
) -> Result<Option<(String, String, String)>> {
    let row = sqlx::query("SELECT actor,state,digest FROM sic_dynamic_toolsets WHERE chain_id=?")
        .bind(chain_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| -> Result<(String, String, String)> {
        Ok((
            row.try_get::<String, _>("actor")?,
            row.try_get::<String, _>("state")?,
            row.try_get::<String, _>("digest")?,
        ))
    })
    .transpose()
    .map_err(Into::into)
}

async fn save_raw(
    tx: &mut Transaction<'_, Sqlite>,
    chain_id: &str,
    actor: &str,
    set: &ToolSet,
    must_exist: bool,
) -> Result<()> {
    let state = set.to_json().map_err(tool_error)?;
    let digest = crate::storage::sha256(state.as_bytes());
    if must_exist {
        let result = sqlx::query(
            "UPDATE sic_dynamic_toolsets SET state=?,digest=?,updated_at=CURRENT_TIMESTAMP WHERE chain_id=? AND actor=?",
        )
        .bind(state)
        .bind(digest)
        .bind(chain_id)
        .bind(actor)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(CoreError::forbidden("toolset actor mismatch"));
        }
    } else {
        sqlx::query(
            "INSERT INTO sic_dynamic_toolsets(chain_id,actor,state,digest) VALUES(?,?,?,?)",
        )
        .bind(chain_id)
        .bind(actor)
        .bind(state)
        .bind(digest)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn validate_scope(chain_id: &str, actor: &str) -> Result<()> {
    if chain_id.trim().is_empty() || actor.trim().is_empty() {
        return Err(CoreError::invalid("toolset chain and actor are required"));
    }
    Ok(())
}

fn tool_error(message: String) -> CoreError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("host-fixed")
        || lower.contains("in-flight")
        || lower.contains("limit")
        || lower.contains("exhausted")
        || lower.contains("cannot be reclaimed")
    {
        CoreError::conflict(message)
    } else if lower.contains("authoriz") || lower.contains("permission") || lower.contains("acl") {
        CoreError::forbidden(message)
    } else if lower.contains("unknown tool") {
        CoreError::new("not_found", message, 404)
    } else {
        CoreError::invalid(message)
    }
}
