//! Durable host-owned task records for historical Socket forks.
//!
//! This module records and arbitrates peer work. A claim is a one-time host
//! execution permit; claimed/running tasks are never automatically reassigned.
//! It coordinates bounded Editor preparation but does not run the Worker or
//! execute external DSH tools.

use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    error::{CoreError, Result},
    storage,
};

const TASKS_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_peer_tasks (
    task_id TEXT PRIMARY KEY NOT NULL,
    actor TEXT NOT NULL,
    requester_chain_id TEXT NOT NULL,
    peer_chain_id TEXT NOT NULL UNIQUE,
    socket_id TEXT NOT NULL,
    checkpoint_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('P','V')),
    state TEXT NOT NULL CHECK(state IN ('pending','claimed','running','prepared','host_reported_completed','failed','unknown')),
    claim_id TEXT,
    claimed_by TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    unknown_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(requester_chain_id, request_id)
)";

const EVENTS_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_peer_task_events (
    task_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY(task_id, event_id),
    FOREIGN KEY(task_id) REFERENCES sic_peer_tasks(task_id) ON DELETE CASCADE
)";

const PREPARES_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_peer_task_prepares (
    task_id TEXT PRIMARY KEY NOT NULL,
    prepare_id TEXT NOT NULL UNIQUE,
    actor TEXT NOT NULL,
    claim_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    running_revision INTEGER NOT NULL,
    policy TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('running','prepared','unknown')),
    result TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(task_id) REFERENCES sic_peer_tasks(task_id) ON DELETE CASCADE
)";

const REPLIES_SQL: &str = "CREATE TABLE IF NOT EXISTS sic_peer_task_replies (
    task_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    reply_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(task_id, attempt_id),
    UNIQUE(task_id, reply_id),
    FOREIGN KEY(task_id) REFERENCES sic_peer_tasks(task_id) ON DELETE CASCADE
)";

pub async fn initialize_in_transaction(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
    sqlx::query(TASKS_SQL).execute(&mut **tx).await?;
    sqlx::query(EVENTS_SQL).execute(&mut **tx).await?;
    sqlx::query(PREPARES_SQL).execute(&mut **tx).await?;
    sqlx::query(REPLIES_SQL).execute(&mut **tx).await?;
    Ok(())
}

pub async fn get_scoped_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
) -> Result<Value> {
    initialize_in_transaction(tx).await?;
    load_scoped(tx, actor, chain_id, task_id).await
}

/// Result of the durable pre-provider CAS. `newly_started=false` is always a
/// replay/current receipt and must never dispatch the scheduler again.
pub struct PrepareAdmission {
    pub newly_started: bool,
    pub task: Value,
    pub prepare_state: String,
    pub running_revision: u64,
    pub historical_result: Option<Value>,
}

pub async fn begin_prepare_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
    prepare_id: &str,
    expected_revision: u64,
    policy: &Value,
) -> Result<PrepareAdmission> {
    validate_key(claim_id, "claim_id")?;
    validate_key(prepare_id, "prepare_id")?;
    if expected_revision >= i64::MAX as u64 {
        return Err(CoreError::invalid(
            "peer task revision exceeds the supported range",
        ));
    }
    initialize_in_transaction(tx).await?;
    let task = load_scoped(tx, actor, chain_id, task_id).await?;
    let fingerprint = storage::digest(&json!({
        "actor":actor,"chain_id":chain_id,"task_id":task_id,"claim_id":claim_id,
        "prepare_id":prepare_id,"expected_revision":expected_revision,"policy":policy
    }))?;
    if let Some(row) = sqlx::query("SELECT prepare_id,actor,claim_id,fingerprint,running_revision,policy,state,result FROM sic_peer_task_prepares WHERE task_id=?")
        .bind(task_id).fetch_optional(&mut **tx).await?
    {
        let old_prepare: String = row.try_get("prepare_id")?;
        let old_actor: String = row.try_get("actor")?;
        let old_claim: String = row.try_get("claim_id")?;
        let old_fingerprint: String = row.try_get("fingerprint")?;
        if old_prepare != prepare_id || old_actor != actor || old_claim != claim_id || old_fingerprint != fingerprint {
            return Err(CoreError::conflict("peer task already has a different prepare request; automatic redispatch is forbidden"));
        }
        let running_revision = u64::try_from(row.try_get::<i64, _>("running_revision")?)
            .map_err(|_| CoreError::new("peer_task_store_error", "stored prepare revision is invalid", 500))?;
        let state: String = row.try_get("state")?;
        let result: Option<String> = row.try_get("result")?;
        let historical_result = result.map(|value| serde_json::from_str(&value)).transpose()?;
        return Ok(PrepareAdmission {
            newly_started: false,
            task,
            prepare_state: state,
            running_revision,
            historical_result,
        });
    }
    if task["state"] != "claimed" || task["claim_id"].as_str() != Some(claim_id) {
        return Err(CoreError::conflict(
            "peer task must be newly claimed by this host before prepare",
        ));
    }
    if task["revision"].as_u64() != Some(expected_revision) {
        return Err(CoreError::conflict(
            "peer task revision changed before prepare",
        ));
    }
    let running_revision = expected_revision + 1;
    let changed = sqlx::query("UPDATE sic_peer_tasks SET state='running',revision=revision+1,unknown_reason=NULL,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND actor=? AND (requester_chain_id=? OR peer_chain_id=?) AND claim_id=? AND state='claimed' AND revision=?")
        .bind(task_id).bind(actor).bind(chain_id).bind(chain_id).bind(claim_id).bind(expected_revision as i64)
        .execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(CoreError::conflict(
            "peer task prepare admission compare-and-swap failed",
        ));
    }
    sqlx::query("INSERT INTO sic_peer_task_prepares(task_id,prepare_id,actor,claim_id,fingerprint,running_revision,policy,state) VALUES(?,?,?,?,?,?,?,'running')")
        .bind(task_id).bind(prepare_id).bind(actor).bind(claim_id).bind(fingerprint)
        .bind(running_revision as i64).bind(storage::json_text(policy)?)
        .execute(&mut **tx).await?;
    let task = load_scoped(tx, actor, chain_id, task_id).await?;
    Ok(PrepareAdmission {
        newly_started: true,
        task,
        prepare_state: "running".into(),
        running_revision,
        historical_result: None,
    })
}

pub async fn finish_prepare_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
    prepare_id: &str,
    running_revision: u64,
    final_state: &str,
    unknown_reason: Option<&str>,
    result: &Value,
) -> Result<Value> {
    if !matches!(final_state, "prepared" | "unknown") {
        return Err(CoreError::invalid(
            "peer prepare state must be prepared or unknown",
        ));
    }
    if running_revision > i64::MAX as u64 || unknown_reason.is_some_and(|value| value.len() > 1024)
    {
        return Err(CoreError::invalid(
            "peer prepare result exceeds supported bounds",
        ));
    }
    initialize_in_transaction(tx).await?;
    let current = load_scoped(tx, actor, chain_id, task_id).await?;
    let prepare = sqlx::query("SELECT claim_id,running_revision,state,result FROM sic_peer_task_prepares WHERE task_id=? AND prepare_id=? AND actor=?")
        .bind(task_id).bind(prepare_id).bind(actor).fetch_optional(&mut **tx).await?;
    let Some(prepare) = prepare else {
        return Err(CoreError::new(
            "not_found",
            "peer task prepare receipt not found",
            404,
        ));
    };
    let stored_claim: String = prepare.try_get("claim_id")?;
    let stored_revision: i64 = prepare.try_get("running_revision")?;
    let stored_state: String = prepare.try_get("state")?;
    if stored_claim != claim_id || stored_revision != running_revision as i64 {
        return Err(CoreError::conflict(
            "peer prepare claim or revision does not match its receipt",
        ));
    }
    if stored_state != "running" {
        let stored_result: Option<String> = prepare.try_get("result")?;
        return stored_result
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .unwrap_or_else(|| {
                Err(CoreError::conflict(
                    "peer prepare receipt is already terminal",
                ))
            });
    }
    let persisted_state = if current["state"] == "running"
        && current["claim_id"].as_str() == Some(claim_id)
        && current["revision"].as_u64() == Some(running_revision)
    {
        let changed = sqlx::query("UPDATE sic_peer_tasks SET state=?,revision=revision+1,unknown_reason=?,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND actor=? AND (requester_chain_id=? OR peer_chain_id=?) AND claim_id=? AND state='running' AND revision=?")
            .bind(final_state).bind(unknown_reason).bind(task_id).bind(actor).bind(chain_id).bind(chain_id)
            .bind(claim_id).bind(running_revision as i64).execute(&mut **tx).await?.rows_affected();
        if changed != 1 {
            return Err(CoreError::conflict(
                "peer prepare finalization compare-and-swap failed",
            ));
        }
        final_state
    } else if current["state"] == "unknown" {
        // An operator may isolate the task while the scheduler is in flight.
        // Preserve that quarantine even if a late result arrives.
        "unknown"
    } else {
        return Err(CoreError::conflict(
            "peer task changed while prepare was running",
        ));
    };
    let mut result = result.clone();
    result["state"] = json!(persisted_state);
    result["task_revision"] =
        json!(current["revision"].as_u64().unwrap_or(0) + u64::from(current["state"] == "running"));
    let serialized = storage::json_text(&result)?;
    sqlx::query("UPDATE sic_peer_task_prepares SET state=?,result=?,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND prepare_id=? AND actor=? AND state='running'")
        .bind(persisted_state).bind(&serialized).bind(task_id).bind(prepare_id).bind(actor)
        .execute(&mut **tx).await?;
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub async fn record_spawn_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    actor: &str,
    requester_chain_id: &str,
    peer_chain_id: &str,
    socket_id: &str,
    checkpoint_id: &str,
    request_id: &str,
    message_id: &str,
    lifecycle: &str,
) -> Result<Value> {
    initialize_in_transaction(tx).await?;
    if lifecycle != "P" && lifecycle != "V" {
        return Err(CoreError::invalid("peer task lifecycle is invalid"));
    }
    let mut record = json!({
        "task_id":task_id,
        "actor":actor,
        "requester_chain_id":requester_chain_id,
        "peer_chain_id":peer_chain_id,
        "socket_id":socket_id,
        "checkpoint_id":checkpoint_id,
        "request_id":request_id,
        "message_id":message_id,
        "lifecycle":lifecycle,
        "state":"pending",
        "claim_id":Value::Null,
        "claimed_by":Value::Null,
        "revision":0,
        "unknown_reason":Value::Null
    });
    sqlx::query("INSERT OR IGNORE INTO sic_peer_tasks(task_id,actor,requester_chain_id,peer_chain_id,socket_id,checkpoint_id,request_id,message_id,lifecycle,state) VALUES(?,?,?,?,?,?,?,?,?,'pending')")
        .bind(task_id).bind(actor).bind(requester_chain_id).bind(peer_chain_id).bind(socket_id)
        .bind(checkpoint_id).bind(request_id).bind(message_id).bind(lifecycle)
        .execute(&mut **tx).await?;
    let existing =
        sqlx::query("SELECT * FROM sic_peer_tasks WHERE requester_chain_id=? AND request_id=?")
            .bind(requester_chain_id)
            .bind(request_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(row) = existing else {
        return Err(CoreError::new(
            "peer_task_store_error",
            "peer task receipt was not persisted",
            500,
        ));
    };
    let current = row_to_value(&row)?;
    // A parent/request replay may only resolve to the same immutable spawn.
    for key in [
        "task_id",
        "actor",
        "peer_chain_id",
        "socket_id",
        "checkpoint_id",
        "message_id",
        "lifecycle",
    ] {
        if current[key] != record[key] {
            return Err(CoreError::conflict(
                "peer task request identity conflicts with existing spawn",
            ));
        }
    }
    record = current;
    Ok(record)
}

pub async fn list_for_chain_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    offset: u64,
    limit: u64,
) -> Result<Vec<Value>> {
    validate_page(offset, limit)?;
    initialize_in_transaction(tx).await?;
    let rows = sqlx::query("SELECT * FROM sic_peer_tasks WHERE actor=? AND (requester_chain_id=? OR peer_chain_id=?) ORDER BY created_at,task_id LIMIT ? OFFSET ?")
        .bind(actor).bind(chain_id).bind(chain_id).bind(limit as i64).bind(offset as i64)
        .fetch_all(&mut **tx).await?;
    let mut tasks = Vec::with_capacity(rows.len());
    for row in rows {
        let mut task = row_to_value(&row)?;
        if let Some(prepare) = sqlx::query(
            "SELECT prepare_id,state,result FROM sic_peer_task_prepares WHERE task_id=?",
        )
        .bind(task["task_id"].as_str().unwrap_or_default())
        .fetch_optional(&mut **tx)
        .await?
        {
            let result: Option<String> = prepare.try_get("result")?;
            task["prepare"] = json!({
                "prepare_id":prepare.try_get::<String,_>("prepare_id")?,
                "state":prepare.try_get::<String,_>("state")?,
                "result":result.map(|value|serde_json::from_str::<Value>(&value)).transpose()?
            });
        }
        tasks.push(task);
    }
    Ok(tasks)
}

pub async fn claim_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
) -> Result<Value> {
    validate_key(claim_id, "claim_id")?;
    initialize_in_transaction(tx).await?;
    let current = load_scoped(tx, actor, chain_id, task_id).await?;
    if current["claim_id"].as_str() == Some(claim_id) {
        let mut replay = current;
        replay["replayed"] = json!(true);
        replay["execution_permit"] = json!(false);
        return Ok(replay);
    }
    if current["state"] != "pending" || !current["claim_id"].is_null() {
        return Err(CoreError::conflict(
            "peer task is already claimed or terminal; it cannot be reassigned",
        ));
    }
    let changed = sqlx::query("UPDATE sic_peer_tasks SET state='claimed',claim_id=?,claimed_by=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND actor=? AND (requester_chain_id=? OR peer_chain_id=?) AND state='pending' AND claim_id IS NULL")
        .bind(claim_id).bind(actor).bind(task_id).bind(actor).bind(chain_id).bind(chain_id)
        .execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(CoreError::conflict(
            "peer task claim lost a concurrent compare-and-swap",
        ));
    }
    let mut result = load_scoped(tx, actor, chain_id, task_id).await?;
    result["replayed"] = json!(false);
    result["execution_permit"] = json!(true);
    Ok(result)
}

pub async fn advance_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
    event_id: &str,
    expected_revision: u64,
    next_state: &str,
) -> Result<Value> {
    if !matches!(next_state, "running" | "completed" | "failed") {
        return Err(CoreError::invalid(
            "peer task advance state must be running, completed, or failed",
        ));
    }
    transition_in_transaction(
        tx,
        actor,
        chain_id,
        task_id,
        claim_id,
        event_id,
        expected_revision,
        next_state,
        None,
    )
    .await
}

pub async fn isolate_unknown_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
    event_id: &str,
    expected_revision: u64,
    reason: &str,
) -> Result<Value> {
    if reason.len() > 1024 {
        return Err(CoreError::invalid(
            "peer task unknown reason exceeds 1024 bytes",
        ));
    }
    transition_in_transaction(
        tx,
        actor,
        chain_id,
        task_id,
        claim_id,
        event_id,
        expected_revision,
        "unknown",
        Some(reason),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn transition_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
    claim_id: &str,
    event_id: &str,
    expected_revision: u64,
    next_state: &str,
    unknown_reason: Option<&str>,
) -> Result<Value> {
    validate_key(claim_id, "claim_id")?;
    validate_key(event_id, "event_id")?;
    if expected_revision > i64::MAX as u64 {
        return Err(CoreError::invalid(
            "peer task revision exceeds the supported range",
        ));
    }
    initialize_in_transaction(tx).await?;
    // Scope authorization precedes receipt lookup so event IDs cannot become
    // a cross-Agent result oracle.
    let current = load_scoped(tx, actor, chain_id, task_id).await?;
    let fingerprint = storage::digest(&json!({
        "actor":actor,"chain_id":chain_id,"task_id":task_id,"claim_id":claim_id,
        "event_id":event_id,"expected_revision":expected_revision,"state":next_state,
        "unknown_reason":unknown_reason
    }))?;
    if let Some(row) = sqlx::query(
        "SELECT fingerprint,result FROM sic_peer_task_events WHERE task_id=? AND event_id=?",
    )
    .bind(task_id)
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        let old: String = row.try_get("fingerprint")?;
        if old != fingerprint {
            return Err(CoreError::conflict(
                "peer task event id was reused with different content",
            ));
        }
        let result: String = row.try_get("result")?;
        let mut value: Value = serde_json::from_str(&result)?;
        value["replayed"] = json!(true);
        value["execution_permit"] = json!(false);
        return Ok(value);
    }
    if current["claim_id"].as_str() != Some(claim_id) {
        return Err(CoreError::forbidden(
            "peer task claim does not match host claim",
        ));
    }
    if current["revision"].as_u64() != Some(expected_revision) {
        return Err(CoreError::conflict("peer task revision changed"));
    }
    let old_state = current["state"].as_str().unwrap_or("");
    let allowed = match next_state {
        "running" => old_state == "claimed",
        "completed" | "failed" => old_state == "running",
        "unknown" => old_state == "claimed" || old_state == "running" || old_state == "prepared",
        _ => false,
    };
    if !allowed {
        return Err(CoreError::conflict("invalid peer task state transition"));
    }
    let persisted_state = if next_state == "completed" {
        "host_reported_completed"
    } else {
        next_state
    };
    let changed = sqlx::query("UPDATE sic_peer_tasks SET state=?,unknown_reason=?,revision=revision+1,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND actor=? AND (requester_chain_id=? OR peer_chain_id=?) AND claim_id=? AND state=? AND revision=?")
        .bind(persisted_state).bind(unknown_reason).bind(task_id).bind(actor).bind(chain_id).bind(chain_id)
        .bind(claim_id).bind(old_state).bind(expected_revision as i64)
        .execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(CoreError::conflict(
            "peer task state compare-and-swap failed",
        ));
    }
    let mut result = load_scoped(tx, actor, chain_id, task_id).await?;
    result["replayed"] = json!(false);
    result["execution_permit"] = json!(false);
    let serialized = storage::json_text(&result)?;
    sqlx::query(
        "INSERT INTO sic_peer_task_events(task_id,event_id,fingerprint,result) VALUES(?,?,?,?)",
    )
    .bind(task_id)
    .bind(event_id)
    .bind(fingerprint)
    .bind(serialized)
    .execute(&mut **tx)
    .await?;
    Ok(result)
}

async fn load_scoped(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
    task_id: &str,
) -> Result<Value> {
    let row = sqlx::query("SELECT * FROM sic_peer_tasks WHERE task_id=? AND actor=? AND (requester_chain_id=? OR peer_chain_id=?)")
        .bind(task_id).bind(actor).bind(chain_id).bind(chain_id)
        .fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Err(CoreError::new(
            "not_found",
            "peer task not found in this Agent scope",
            404,
        ));
    };
    row_to_value(&row)
}

fn row_to_value(row: &sqlx::sqlite::SqliteRow) -> Result<Value> {
    let mut value = json!({
        "task_id":row.try_get::<String,_>("task_id")?,
        "actor":row.try_get::<String,_>("actor")?,
        "requester_chain_id":row.try_get::<String,_>("requester_chain_id")?,
        "peer_chain_id":row.try_get::<String,_>("peer_chain_id")?,
        "socket_id":row.try_get::<String,_>("socket_id")?,
        "checkpoint_id":row.try_get::<String,_>("checkpoint_id")?,
        "request_id":row.try_get::<String,_>("request_id")?,
        "message_id":row.try_get::<String,_>("message_id")?,
        "lifecycle":row.try_get::<String,_>("lifecycle")?,
        "state":row.try_get::<String,_>("state")?,
        "claim_id":row.try_get::<Option<String>,_>("claim_id")?,
        "claimed_by":row.try_get::<Option<String>,_>("claimed_by")?,
        "revision":u64::try_from(row.try_get::<i64,_>("revision")?).unwrap_or(0),
        "unknown_reason":row.try_get::<Option<String>,_>("unknown_reason")?,
        "created_at":row.try_get::<String,_>("created_at")?,
        "updated_at":row.try_get::<String,_>("updated_at")?
    });
    if value["state"] == "host_reported_completed" {
        value["completion_attestation"] = json!({
            "kind":"host_only",
            "evidence_verified":false
        });
    }
    Ok(value)
}

fn validate_key(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(CoreError::invalid(format!(
            "{field} must be nonempty and at most 128 bytes"
        )));
    }
    Ok(())
}

fn validate_page(offset: u64, limit: u64) -> Result<()> {
    if limit == 0 || limit > 100 || offset > 100_000 {
        return Err(CoreError::invalid(
            "peer task page must use limit 1..100 and offset at most 100000",
        ));
    }
    Ok(())
}
