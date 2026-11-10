//! Narrow, audited restoration of configuration fields with equivalent Rust
//! validation. This module never resumes a legacy queue or authorizes a model.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{
    error::{CoreError, Result},
    legacy::{LegacyRawSnapshot, RawTable},
    storage::{self, Store},
};

const RECEIPT_TABLE: &str = "legacy_config_restore_receipts";
fn rust_default_feedback() -> Value {
    json!({"max_corrections":2,"feedback_chars":8192})
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetGuard {
    pub run_state: String,
    pub cut_seq: u64,
    pub control_epoch: u64,
    pub input_epoch: u64,
    pub config_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoredField {
    pub name: String,
    pub value_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreReport {
    pub target_chain_id: String,
    pub source_chain_id: String,
    pub source_archive_sha256: String,
    pub target_config_sha256_before: String,
    pub target_config_sha256_after: String,
    pub cut_seq: u64,
    pub fields: Vec<RestoredField>,
    pub replayed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigAuditReport {
    pub target_chain_id: String,
    pub source_chain_id: String,
    pub source_archive_sha256: String,
    pub target_guard: TargetGuard,
    pub fields: Vec<RestoredField>,
    pub eligible: bool,
}

fn refused(message: &'static str) -> CoreError {
    CoreError::new("legacy_config_refused", message, 409)
}

fn from_hex(text: &str) -> Result<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err(refused("legacy config archive contains malformed raw text"));
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            let high = digit(pair[0])
                .ok_or_else(|| refused("legacy config archive contains malformed raw text"))?;
            let low = digit(pair[1])
                .ok_or_else(|| refused("legacy config archive contains malformed raw text"))?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn text_cell(table: &RawTable, row: usize, field: &str) -> Result<String> {
    let index = table
        .columns
        .iter()
        .position(|column| column == field)
        .ok_or_else(|| refused("legacy source table has an unsupported shape"))?;
    let cell = table
        .rows
        .get(row)
        .and_then(|cells| cells.get(index))
        .ok_or_else(|| refused("legacy source row has an unsupported shape"))?;
    if cell.storage_class != "TEXT" {
        return Err(refused("legacy config source field is not text"));
    }
    let raw = cell
        .value
        .get("hex")
        .and_then(Value::as_str)
        .ok_or_else(|| refused("legacy config source field is unavailable"))?;
    String::from_utf8(from_hex(raw)?)
        .map_err(|_| refused("legacy config source is not valid UTF-8"))
}

fn verify_snapshot(snapshot: &LegacyRawSnapshot) -> Result<()> {
    snapshot
        .verify()
        .map_err(|_| refused("legacy source archive digest is invalid"))?;
    if snapshot.schema != "sic-python-raw-record-snapshot-v1"
        || !snapshot.raw_only
        || snapshot.executable_import
        || snapshot.original_modified
    {
        return Err(refused(
            "legacy source archive is not a read-only raw snapshot",
        ));
    }
    for table in &snapshot.tables {
        if table.rows.len() != table.row_ids.len() || table.rows.len() != table.row_sha256.len() {
            return Err(refused("legacy source archive row digests are incomplete"));
        }
        for ((row, row_id), expected) in
            table.rows.iter().zip(&table.row_ids).zip(&table.row_sha256)
        {
            let encoded = serde_json::to_vec(&(row_id, row))
                .map_err(|_| refused("legacy source archive cannot be verified"))?;
            if storage::sha256(&encoded) != *expected {
                return Err(refused("legacy source archive row digest is invalid"));
            }
        }
    }
    Ok(())
}

fn source_config(snapshot: &LegacyRawSnapshot, actor: &str) -> Result<(String, Value)> {
    let table = snapshot
        .tables
        .iter()
        .find(|table| table.name == "chains")
        .ok_or_else(|| refused("legacy source archive has no Agent table"))?;
    if table.columns != ["id", "state"] || table.rows.len() != 1 {
        return Err(refused(
            "legacy source archive does not contain exactly one supported Agent",
        ));
    }
    let source_chain_id = text_cell(table, 0, "id")?;
    let source_state_text = text_cell(table, 0, "state")?;
    let source_state: Value = serde_json::from_str(&source_state_text)
        .map_err(|_| refused("legacy source Agent state is malformed"))?;
    let owners = source_state
        .get("owners")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("legacy source owner mapping is unavailable"))?;
    let unique: BTreeSet<&str> = owners.iter().filter_map(Value::as_str).collect();
    if owners.len() != 1 || unique.len() != 1 || unique.first().copied() != Some(actor) {
        return Err(refused(
            "legacy source Agent does not have exactly this authenticated owner",
        ));
    }
    if source_state.get("run_state").and_then(Value::as_str) != Some("running") {
        return Err(refused(
            "legacy source Agent is outside the verified upgrade shape",
        ));
    }
    let config = source_state
        .get("config")
        .filter(|value| value.is_object())
        .ok_or_else(|| refused("legacy source config is unavailable"))?;
    let mut selected = serde_json::Map::new();
    if let Some(goal) = config.get("task_goal") {
        if !goal.is_string() {
            return Err(refused("legacy task_goal fails Rust config validation"));
        }
        selected.insert("task_goal".into(), goal.clone());
    }
    if let Some(feedback) = config.get("editor_feedback") {
        validate_editor_feedback(feedback)?;
        selected.insert("editor_feedback".into(), feedback.clone());
    }
    if selected.is_empty() {
        return Err(refused("legacy source contains no supported config fields"));
    }
    Ok((source_chain_id, Value::Object(selected)))
}

fn validate_editor_feedback(value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| refused("legacy editor_feedback fails Rust config validation"))?;
    if object.len() != 2
        || object
            .keys()
            .any(|key| key != "max_corrections" && key != "feedback_chars")
    {
        return Err(refused("legacy editor_feedback has unsupported fields"));
    }
    let corrections = object.get("max_corrections").and_then(Value::as_u64);
    let chars = object.get("feedback_chars").and_then(Value::as_u64);
    if corrections.is_none_or(|value| value > 3)
        || chars.is_none_or(|value| !(512..=32768).contains(&value))
    {
        return Err(refused("legacy editor_feedback fails Rust config bounds"));
    }
    Ok(())
}

fn config_digest(config: &Value) -> Result<String> {
    Ok(storage::sha256(storage::json_text(config)?.as_bytes()))
}

fn guard_from_agent(agent: &Value) -> Result<TargetGuard> {
    let config = agent
        .get("config")
        .filter(|value| value.is_object())
        .ok_or_else(|| refused("target Agent config is unavailable"))?;
    Ok(TargetGuard {
        run_state: agent
            .get("run_state")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("target Agent run state is unavailable"))?
            .to_owned(),
        cut_seq: agent
            .get("cut_seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| refused("target Agent revision is unavailable"))?,
        control_epoch: agent
            .get("control_epoch")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        input_epoch: agent
            .get("input_epoch")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        config_sha256: config_digest(config)?,
    })
}

fn verify_owner(agent: &Value, actor: &str) -> Result<()> {
    let owners = agent
        .get("owners")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("target Agent owner mapping is unavailable"))?;
    if owners.len() != 1 || owners[0].as_str() != Some(actor) {
        return Err(CoreError::forbidden(
            "target Agent is unavailable to this actor",
        ));
    }
    Ok(())
}

async fn read_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &str,
    chain_id: &str,
) -> Result<Value> {
    let row = sqlx::query("SELECT state FROM chains WHERE id=?")
        .bind(chain_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| CoreError::new("not_found", "target Agent not found", 404))?;
    let state: String = row.try_get("state")?;
    let agent: Value =
        serde_json::from_str(&state).map_err(|_| refused("target Agent state is malformed"))?;
    verify_owner(&agent, actor)?;
    Ok(agent)
}

/// Read only the target revision/status/config digest. No config values are returned.
pub async fn read_target_guard(store: &Store, actor: &str, chain_id: &str) -> Result<TargetGuard> {
    if actor.trim().is_empty() || chain_id.trim().is_empty() {
        return Err(CoreError::invalid(
            "authenticated actor and target Agent are required",
        ));
    }
    let mut tx = store.begin().await?;
    let agent = read_target(&mut tx, actor, chain_id).await?;
    let guard = guard_from_agent(&agent)?;
    tx.commit().await?;
    Ok(guard)
}

fn digest_field(value: &Value) -> Result<String> {
    Ok(storage::sha256(
        serde_json::to_vec(value)
            .map_err(|_| CoreError::invalid("legacy config digest cannot be computed"))?
            .as_slice(),
    ))
}

async fn archived_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor: &str,
    target_chain_id: &str,
) -> Result<(String, String, Value)> {
    let rows =
        sqlx::query("SELECT data,digest FROM objects WHERE kind='legacy_python_upgrade_manifest'")
            .fetch_all(&mut **tx)
            .await?;
    let mut matching = Vec::new();
    for row in rows {
        let data: String = row.try_get("data")?;
        let digest: String = row.try_get("digest")?;
        if storage::sha256(data.as_bytes()) != digest {
            return Err(refused("legacy migration manifest digest is invalid"));
        }
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| refused("legacy migration manifest is malformed"))?;
        if value
            .get("target_agent")
            .and_then(|item| item.get("chain_id"))
            .and_then(Value::as_str)
            == Some(target_chain_id)
        {
            matching.push(value);
        }
    }
    if matching.len() != 1 {
        return Err(refused(
            "target Agent must have exactly one verified legacy migration manifest",
        ));
    }
    let manifest = &matching[0];
    if manifest.get("schema").and_then(Value::as_str)
        != Some("sic-python-active-upgrade-manifest-v1")
    {
        return Err(refused("legacy migration manifest schema is unsupported"));
    }
    let source_archive_sha256 = manifest
        .get("source_archive_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| refused("legacy source archive digest is missing"))?
        .to_owned();
    if source_archive_sha256.len() != 64
        || !source_archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(refused("legacy source archive digest is malformed"));
    }
    let archives =
        sqlx::query("SELECT data,digest FROM objects WHERE kind='legacy_python_raw_snapshot'")
            .fetch_all(&mut **tx)
            .await?;
    let mut matching_snapshot = None;
    let mut matching_count = 0usize;
    for row in archives {
        let archive_data: String = row.try_get("data")?;
        let archive_object_digest: String = row.try_get("digest")?;
        if storage::sha256(archive_data.as_bytes()) != archive_object_digest {
            return Err(refused("legacy source archive object digest is invalid"));
        }
        let snapshot: LegacyRawSnapshot = serde_json::from_str(&archive_data)
            .map_err(|_| refused("legacy source archive is malformed"))?;
        if snapshot.archive_sha256 == source_archive_sha256 {
            matching_count += 1;
            matching_snapshot = Some(snapshot);
        }
    }
    if matching_count != 1 {
        return Err(refused(
            "legacy source archive reference is missing or ambiguous",
        ));
    }
    let snapshot = matching_snapshot.unwrap();
    verify_snapshot(&snapshot)?;
    let (source_chain_id, selected) = source_config(&snapshot, actor)?;
    if manifest
        .get("original_agent")
        .and_then(|value| value.get("chain_id"))
        .and_then(Value::as_str)
        != Some(&source_chain_id)
        || manifest
            .get("original_agent")
            .and_then(|value| value.get("owners"))
            .and_then(Value::as_array)
            .is_none_or(|owners| owners.len() != 1 || owners[0].as_str() != Some(actor))
    {
        return Err(refused(
            "legacy manifest owner/source mapping differs from the verified archive",
        ));
    }
    let source_state_text = text_cell(
        snapshot
            .tables
            .iter()
            .find(|table| table.name == "chains")
            .unwrap(),
        0,
        "state",
    )?;
    let source_state_digest = storage::sha256(source_state_text.as_bytes());
    if manifest
        .get("original_agent")
        .and_then(|value| value.get("state_sha256"))
        .and_then(Value::as_str)
        != Some(source_state_digest.as_str())
    {
        return Err(refused(
            "legacy source Agent digest differs from the manifest",
        ));
    }
    Ok((source_chain_id, source_archive_sha256, selected))
}

fn validate_target_is_idle(agent: &Value) -> Result<()> {
    if agent.get("run_state").and_then(Value::as_str) != Some("stopped")
        || agent.get("cut_seq").and_then(Value::as_u64) != Some(0)
        || agent
            .get("control_epoch")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            != 0
        || agent
            .get("input_epoch")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            != 0
        || agent
            .get("worker_claim")
            .is_some_and(|value| !value.is_null())
        || agent
            .get("current_cut")
            .is_some_and(|value| !value.is_null())
        || agent
            .get("events")
            .and_then(Value::as_array)
            .is_none_or(|items| !items.is_empty())
        || agent
            .get("jobs")
            .and_then(Value::as_object)
            .is_none_or(|jobs| !jobs.is_empty())
    {
        return Err(refused(
            "target Agent is not the stopped, empty imported state",
        ));
    }
    let contexts = agent
        .get("contexts")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("target Agent contexts are unavailable"))?;
    for context in contexts.values() {
        if !context
            .get("shared_contexts")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            return Err(refused("target Agent contains nonempty context work"));
        }
        let levels = context
            .get("levels")
            .and_then(Value::as_object)
            .ok_or_else(|| refused("target Agent levels are unavailable"))?;
        for level in levels.values() {
            for key in ["wbs", "cbs", "pending"] {
                if !level
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
                {
                    return Err(refused("target Agent contains nonempty context work"));
                }
            }
        }
    }
    Ok(())
}

fn validate_field_compatibility(target: &Value, selected: &Value) -> Result<()> {
    let config = target
        .get("config")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("target Agent config is unavailable"))?;
    if selected.get("task_goal").is_some_and(|source_goal| {
        config
            .get("task_goal")
            .is_some_and(|current| current != source_goal)
    }) {
        return Err(refused(
            "target task_goal has a different host value; refusing overwrite",
        ));
    }
    if let Some(source_feedback) = selected.get("editor_feedback") {
        let current = config
            .get("editor_feedback")
            .ok_or_else(|| refused("target editor_feedback is unavailable"))?;
        if current != &rust_default_feedback() && current != source_feedback {
            return Err(refused(
                "target editor_feedback has a different host value; refusing overwrite",
            ));
        }
    }
    Ok(())
}

async fn verify_quarantined_queue(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    chain_id: &str,
    manifest: &Value,
) -> Result<()> {
    let expected = manifest
        .pointer("/runtime_queue/quarantined_rows")
        .and_then(Value::as_u64)
        .ok_or_else(|| refused("legacy manifest queue summary is unavailable"))?;
    let rows = sqlx::query("SELECT status,event FROM runtime_queue WHERE chain_id=?")
        .bind(chain_id)
        .fetch_all(&mut **tx)
        .await?;
    if rows.len() as u64 != expected {
        return Err(refused(
            "target runtime queue differs from the quarantined migration manifest",
        ));
    }
    for row in rows {
        let status: String = row.try_get("status")?;
        let event_text: String = row.try_get("event")?;
        let event: Value = serde_json::from_str(&event_text)
            .map_err(|_| refused("quarantined runtime event is malformed"))?;
        if status != "legacy-quarantined"
            || event.get("kind").and_then(Value::as_str) != Some("legacy_runtime_event_quarantined")
            || event.get("execution").and_then(Value::as_str)
                != Some("disabled; row is not eligible for scheduling")
        {
            return Err(refused(
                "target runtime queue contains a non-quarantined event",
            ));
        }
    }
    Ok(())
}

fn restore_request_fingerprint(
    actor: &str,
    target_chain_id: &str,
    source_archive_sha256: &str,
    guard: &TargetGuard,
    fields: &[RestoredField],
) -> Result<String> {
    let value = json!({"actor":actor,"target_chain_id":target_chain_id,
        "source_archive_sha256":source_archive_sha256,"guard":guard,"fields":fields});
    Ok(storage::sha256(&serde_json::to_vec(&value).map_err(
        |_| CoreError::invalid("legacy config request cannot be fingerprinted"),
    )?))
}

/// Restore only validated `task_goal` and `editor_feedback` values. Caller must
/// provide the read-only target guard returned by `read_target_guard`; retries
/// with the same key replay a digest-only receipt and never repeat writes.
pub async fn restore_legacy_config(
    store: &Store,
    authenticated_actor: &str,
    target_chain_id: &str,
    idempotency_key: &str,
    expected: &TargetGuard,
) -> Result<RestoreReport> {
    if authenticated_actor.trim().is_empty()
        || target_chain_id.trim().is_empty()
        || idempotency_key.trim().is_empty()
        || idempotency_key.len() > 128
        || !idempotency_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(CoreError::invalid(
            "authenticated actor, target Agent and bounded idempotency key are required",
        ));
    }
    let mut tx = store.begin().await?;
    let agent = read_target(&mut tx, authenticated_actor, target_chain_id).await?;
    sqlx::query(&format!("CREATE TABLE IF NOT EXISTS {RECEIPT_TABLE}(target_chain_id TEXT NOT NULL,idempotency_key TEXT NOT NULL,actor TEXT NOT NULL,request_fingerprint TEXT NOT NULL,receipt TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(target_chain_id,idempotency_key))"))
        .execute(&mut *tx).await?;
    if let Some(row) = sqlx::query(&format!("SELECT actor,request_fingerprint,receipt FROM {RECEIPT_TABLE} WHERE target_chain_id=? AND idempotency_key=?"))
        .bind(target_chain_id).bind(idempotency_key).fetch_optional(&mut *tx).await? {
        let actor: String = row.try_get("actor")?;
        let saved_fingerprint: String = row.try_get("request_fingerprint")?;
        let saved_receipt: String = row.try_get("receipt")?;
        if actor != authenticated_actor {
            return Err(CoreError::forbidden("legacy config receipt belongs to another actor"));
        }
        let (_source_chain_id, archive_digest, selected) = archived_source(&mut tx, authenticated_actor, target_chain_id).await?;
        let fields = selected.as_object().unwrap().iter().map(|(name, value)| Ok(RestoredField {
            name: name.clone(), value_sha256: digest_field(value)?,
        })).collect::<Result<Vec<_>>>()?;
        let request_fingerprint = restore_request_fingerprint(authenticated_actor, target_chain_id, &archive_digest, expected, &fields)?;
        if saved_fingerprint != request_fingerprint {
            return Err(refused("idempotency key was already used for another config restore request"));
        }
        // Ensure this actor still owns the target, but do not reapply or require
        // its current revision to match after a successful prior restore.
        verify_owner(&agent, authenticated_actor)?;
        let mut receipt: RestoreReport = serde_json::from_str(&saved_receipt)
            .map_err(|_| refused("legacy config receipt is malformed"))?;
        receipt.replayed = true;
        tx.commit().await?;
        return Ok(receipt);
    }

    let (source_chain_id, source_archive_sha256, selected) =
        archived_source(&mut tx, authenticated_actor, target_chain_id).await?;
    let fields = selected
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| {
            Ok(RestoredField {
                name: name.clone(),
                value_sha256: digest_field(value)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let request_fingerprint = restore_request_fingerprint(
        authenticated_actor,
        target_chain_id,
        &source_archive_sha256,
        expected,
        &fields,
    )?;
    let actual_guard = guard_from_agent(&agent)?;
    if &actual_guard != expected {
        return Err(refused(
            "target Agent revision, status, or config changed since it was reviewed",
        ));
    }
    validate_target_is_idle(&agent)?;
    validate_field_compatibility(&agent, &selected)?;
    let manifest = manifest_for_target(&mut tx, target_chain_id).await?;
    verify_quarantined_queue(&mut tx, target_chain_id, &manifest).await?;

    let mut config = agent
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| refused("target Agent config is unavailable"))?;
    if let Some(source_goal) = selected.get("task_goal") {
        config.insert("task_goal".into(), source_goal.clone());
    }
    if let Some(source_feedback) = selected.get("editor_feedback") {
        config.insert("editor_feedback".into(), source_feedback.clone());
    }
    let before = actual_guard.config_sha256.clone();
    let config_value = Value::Object(config);
    let after = config_digest(&config_value)?;
    let mut new_agent = agent;
    new_agent["config"] = config_value;
    let state_text = storage::json_text(&new_agent)?;
    let update = sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(&state_text)
        .bind(target_chain_id)
        .execute(&mut *tx)
        .await?;
    if update.rows_affected() != 1 {
        return Err(refused(
            "target Agent changed before config could be restored",
        ));
    }
    let receipt = RestoreReport {
        target_chain_id: target_chain_id.into(),
        source_chain_id,
        source_archive_sha256,
        target_config_sha256_before: before,
        target_config_sha256_after: after,
        cut_seq: actual_guard.cut_seq,
        fields,
        replayed: false,
    };
    let receipt_text = storage::json_text(
        &serde_json::to_value(&receipt)
            .map_err(|_| CoreError::invalid("legacy config receipt cannot be serialized"))?,
    )?;
    sqlx::query(&format!("INSERT INTO {RECEIPT_TABLE}(target_chain_id,idempotency_key,actor,request_fingerprint,receipt,created) VALUES(?,?,?,?,?,?)"))
        .bind(target_chain_id).bind(idempotency_key).bind(authenticated_actor)
        .bind(request_fingerprint).bind(receipt_text).bind(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(receipt)
}

async fn manifest_for_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    chain_id: &str,
) -> Result<Value> {
    let rows =
        sqlx::query("SELECT data,digest FROM objects WHERE kind='legacy_python_upgrade_manifest'")
            .fetch_all(&mut **tx)
            .await?;
    let mut found = Vec::new();
    for row in rows {
        let data: String = row.try_get("data")?;
        let digest: String = row.try_get("digest")?;
        if storage::sha256(data.as_bytes()) != digest {
            return Err(refused("legacy migration manifest digest is invalid"));
        }
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| refused("legacy migration manifest is malformed"))?;
        if value
            .pointer("/target_agent/chain_id")
            .and_then(Value::as_str)
            == Some(chain_id)
        {
            found.push(value);
        }
    }
    if found.len() != 1 {
        return Err(refused(
            "target Agent must have exactly one verified legacy migration manifest",
        ));
    }
    Ok(found.remove(0))
}

/// Read-only preflight for the narrow config restore. The report exposes only
/// target guards, field names, source provenance, and digests.
pub async fn audit_legacy_config(
    store: &Store,
    authenticated_actor: &str,
    target_chain_id: &str,
) -> Result<ConfigAuditReport> {
    if authenticated_actor.trim().is_empty() || target_chain_id.trim().is_empty() {
        return Err(CoreError::invalid(
            "authenticated actor and target Agent are required",
        ));
    }
    let mut tx = store.begin().await?;
    let agent = read_target(&mut tx, authenticated_actor, target_chain_id).await?;
    let (source_chain_id, source_archive_sha256, selected) =
        archived_source(&mut tx, authenticated_actor, target_chain_id).await?;
    let guard = guard_from_agent(&agent)?;
    validate_target_is_idle(&agent)?;
    validate_field_compatibility(&agent, &selected)?;
    let manifest = manifest_for_target(&mut tx, target_chain_id).await?;
    verify_quarantined_queue(&mut tx, target_chain_id, &manifest).await?;
    let fields = selected
        .as_object()
        .ok_or_else(|| refused("legacy config source is malformed"))?
        .iter()
        .map(|(name, value)| {
            Ok(RestoredField {
                name: name.clone(),
                value_sha256: digest_field(value)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(ConfigAuditReport {
        target_chain_id: target_chain_id.into(),
        source_chain_id,
        source_archive_sha256,
        target_guard: guard,
        fields,
        eligible: true,
    })
}
