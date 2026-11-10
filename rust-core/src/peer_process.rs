//! One-shot host-owned process launch primitive for prepared peer tasks.
//!
//! This module does not interpret DSH messages or execute model-provided
//! commands. The host supplies an explicit program, argv, environment allowlist
//! and finite timeout. A durable per-task intent is committed before spawn;
//! ambiguous outcomes are never retried.

use crate::{
    core::Core,
    error::{CoreError, Result},
    peer_tasks,
    storage::{self, Store},
};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const LAUNCH_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS sic_peer_process_launches (
 task_id TEXT PRIMARY KEY NOT NULL, launch_id TEXT NOT NULL UNIQUE, actor TEXT NOT NULL,
 peer_chain_id TEXT NOT NULL, claim_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
 executable TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('intent','running','exited','unknown')),
 exit_code INTEGER, reason TEXT, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
)";

/// Host-selected, immutable process parameters. Never construct this from a
/// model/tool payload. `env_clear` is always applied before these entries.
#[derive(Clone)]
pub struct HostProcessSpec {
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub environment: BTreeMap<String, String>,
    /// An optional host-selected working directory. It is never chosen by the model.
    pub working_directory: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchReceipt {
    pub task_id: String,
    pub launch_id: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub replayed: bool,
    /// True only when the host process exited. This is not a tool-success or
    /// peer-task-completion attestation.
    pub process_exited: bool,
}

/// Validate a host process specification without granting a launch right.
pub fn validate_host_process_spec(spec: &HostProcessSpec) -> Result<()> {
    validate_spec(spec)?;
    let executable = std::fs::canonicalize(&spec.executable)
        .map_err(|_| CoreError::invalid("host executable cannot be resolved"))?;
    validate_program(&executable)
}

/// Reserves this task's sole launch right transactionally, commits, then starts
/// the host-configured executable directly (never through a shell).
pub async fn launch_prepared_peer(
    store: &Store,
    actor: &str,
    peer_chain_id: &str,
    task_id: &str,
    claim_id: &str,
    launch_id: &str,
    spec: &HostProcessSpec,
) -> Result<LaunchReceipt> {
    validate_key(actor, "actor")?;
    validate_key(peer_chain_id, "peer_chain_id")?;
    validate_key(task_id, "task_id")?;
    validate_key(claim_id, "claim_id")?;
    validate_key(launch_id, "launch_id")?;
    validate_spec(spec)?;
    let executable = std::fs::canonicalize(&spec.executable)
        .map_err(|_| CoreError::invalid("host executable cannot be resolved"))?;
    validate_program(&executable)?;
    let fingerprint = spec_fingerprint(&executable, spec)?;
    let core = Core::new(store.clone());

    let mut tx = store.begin().await?;
    let admitted = reserve_in_transaction(
        &core,
        &mut tx,
        actor,
        peer_chain_id,
        task_id,
        claim_id,
        launch_id,
        &executable,
        &fingerprint,
    )
    .await?;
    tx.commit().await?;
    if let Some(receipt) = admitted {
        return Ok(receipt);
    }

    // The committed intent is the irrevocable one-shot admission. Even a spawn
    // error or cancellation remains unknown and cannot be automatically retried.
    let mut command = Command::new(&executable);
    command
        .args(&spec.argv)
        .env_clear()
        .envs(&spec.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The host supervisor is noninteractive and should not open a console.
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    if let Some(directory) = &spec.working_directory {
        command.current_dir(directory);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            set_state(
                store,
                actor,
                peer_chain_id,
                task_id,
                launch_id,
                "unknown",
                None,
                Some("spawn_failed"),
            )
            .await?;
            return load_receipt(store, actor, peer_chain_id, task_id, launch_id, false).await;
        }
    };
    set_state(
        store,
        actor,
        peer_chain_id,
        task_id,
        launch_id,
        "running",
        None,
        None,
    )
    .await?;

    match tokio::time::timeout(spec.timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let code = status.code();
            set_state(
                store,
                actor,
                peer_chain_id,
                task_id,
                launch_id,
                "exited",
                code,
                None,
            )
            .await?;
        }
        Ok(Err(_)) => {
            set_state(
                store,
                actor,
                peer_chain_id,
                task_id,
                launch_id,
                "unknown",
                None,
                Some("wait_failed"),
            )
            .await?;
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            set_state(
                store,
                actor,
                peer_chain_id,
                task_id,
                launch_id,
                "unknown",
                None,
                Some("timeout"),
            )
            .await?;
        }
    }
    load_receipt(store, actor, peer_chain_id, task_id, launch_id, false).await
}

/// Explicit startup reconciliation. Call only while the host has exclusive
/// ownership of process recovery; never launches anything. A prior `intent` or
/// `running` is conservatively recorded unknown, because process outcome may
/// have become ambiguous across a crash.
pub async fn mark_interrupted_unknown(
    store: &Store,
    actor: &str,
    peer_chain_id: &str,
    task_id: &str,
    launch_id: &str,
) -> Result<LaunchReceipt> {
    validate_key(actor, "actor")?;
    validate_key(peer_chain_id, "peer_chain_id")?;
    validate_key(task_id, "task_id")?;
    validate_key(launch_id, "launch_id")?;
    let mut tx = store.begin().await?;
    let _ = peer_tasks::get_scoped_in_transaction(&mut tx, actor, peer_chain_id, task_id).await?;
    verify_chain_owner(&mut tx, actor, peer_chain_id).await?;
    let changed = sqlx::query("UPDATE sic_peer_process_launches SET state='unknown',reason='interrupted',updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND launch_id=? AND actor=? AND peer_chain_id=? AND state IN ('intent','running')")
        .bind(task_id).bind(launch_id).bind(actor).bind(peer_chain_id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        let exists = sqlx::query("SELECT 1 FROM sic_peer_process_launches WHERE task_id=? AND launch_id=? AND actor=? AND peer_chain_id=?")
            .bind(task_id).bind(launch_id).bind(actor).bind(peer_chain_id).fetch_optional(&mut *tx).await?.is_some();
        if !exists {
            return Err(CoreError::new(
                "not_found",
                "process launch receipt not found",
                404,
            ));
        }
    }
    tx.commit().await?;
    load_receipt(store, actor, peer_chain_id, task_id, launch_id, true).await
}

/// Inspect a previously recorded launch without granting permission to start
/// one. `None` means no process intent exists for this task; it is not proof
/// that a lost peer-task claim may safely be dispatched again.
pub async fn inspect_launch(
    store: &Store,
    actor: &str,
    peer_chain_id: &str,
    task_id: &str,
    claim_id: &str,
    launch_id: &str,
) -> Result<Option<LaunchReceipt>> {
    inspect_launch_inner(
        store,
        actor,
        peer_chain_id,
        task_id,
        claim_id,
        launch_id,
        None,
    )
    .await
}

/// Inspect without spawning, while also requiring the exact host process
/// specification used for the durable intent.
pub async fn inspect_launch_with_spec(
    store: &Store,
    actor: &str,
    peer_chain_id: &str,
    task_id: &str,
    claim_id: &str,
    launch_id: &str,
    spec: &HostProcessSpec,
) -> Result<Option<LaunchReceipt>> {
    validate_host_process_spec(spec)?;
    let executable = std::fs::canonicalize(&spec.executable)
        .map_err(|_| CoreError::invalid("host executable cannot be resolved"))?;
    let fingerprint = spec_fingerprint(&executable, spec)?;
    inspect_launch_inner(
        store,
        actor,
        peer_chain_id,
        task_id,
        claim_id,
        launch_id,
        Some(&fingerprint),
    )
    .await
}

async fn inspect_launch_inner(
    store: &Store,
    actor: &str,
    peer_chain_id: &str,
    task_id: &str,
    claim_id: &str,
    launch_id: &str,
    expected_fingerprint: Option<&str>,
) -> Result<Option<LaunchReceipt>> {
    for (value, name) in [
        (actor, "actor"),
        (peer_chain_id, "peer_chain_id"),
        (task_id, "task_id"),
        (claim_id, "claim_id"),
        (launch_id, "launch_id"),
    ] {
        validate_key(value, name)?;
    }
    let mut tx = store.begin().await?;
    let task =
        peer_tasks::get_scoped_in_transaction(&mut tx, actor, peer_chain_id, task_id).await?;
    if task["peer_chain_id"].as_str() != Some(peer_chain_id) {
        return Err(CoreError::forbidden(
            "peer task does not belong to this peer route",
        ));
    }
    verify_chain_owner(&mut tx, actor, peer_chain_id).await?;
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sic_peer_process_launches'",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.commit().await?;
        return Ok(None);
    }
    let row = sqlx::query("SELECT launch_id,actor,peer_chain_id,claim_id,fingerprint,state,exit_code FROM sic_peer_process_launches WHERE task_id=?")
        .bind(task_id).fetch_optional(&mut *tx).await?;
    let result = if let Some(row) = row {
        let old_actor: String = row.try_get("actor")?;
        let old_peer: String = row.try_get("peer_chain_id")?;
        let old_claim: String = row.try_get("claim_id")?;
        let old_launch: String = row.try_get("launch_id")?;
        let old_fingerprint: String = row.try_get("fingerprint")?;
        if old_actor != actor
            || old_peer != peer_chain_id
            || old_claim != claim_id
            || old_launch != launch_id
            || expected_fingerprint.is_some_and(|fingerprint| fingerprint != old_fingerprint)
        {
            return Err(CoreError::conflict(
                "peer task has a different process launch identity",
            ));
        }
        Some(row_receipt(&row, task_id, true)?)
    } else {
        None
    };
    tx.commit().await?;
    Ok(result)
}

async fn reserve_in_transaction(
    core: &Core,
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    peer: &str,
    task_id: &str,
    claim_id: &str,
    launch_id: &str,
    executable: &Path,
    fingerprint: &str,
) -> Result<Option<LaunchReceipt>> {
    peer_tasks::initialize_in_transaction(tx).await?;
    sqlx::query(LAUNCH_SCHEMA).execute(&mut **tx).await?;
    let task = peer_tasks::get_scoped_in_transaction(tx, actor, peer, task_id).await?;
    if task["peer_chain_id"].as_str() != Some(peer) {
        return Err(CoreError::forbidden(
            "peer task does not belong to this peer route",
        ));
    }
    verify_chain_owner(tx, actor, peer).await?;

    if let Some(row) = sqlx::query("SELECT launch_id,actor,peer_chain_id,claim_id,fingerprint,state,exit_code FROM sic_peer_process_launches WHERE task_id=?")
        .bind(task_id).fetch_optional(&mut **tx).await? {
        let old_launch: String = row.try_get("launch_id")?;
        let old_actor: String = row.try_get("actor")?;
        let old_peer: String = row.try_get("peer_chain_id")?;
        let old_claim: String = row.try_get("claim_id")?;
        let old_fingerprint: String = row.try_get("fingerprint")?;
        if old_launch != launch_id || old_actor != actor || old_peer != peer || old_claim != claim_id || old_fingerprint != fingerprint {
            return Err(CoreError::conflict("peer task already has a different process launch intent; retry is forbidden"));
        }
        return Ok(Some(row_receipt(&row, task_id, true)?));
    }

    core.verify_peer_process_launch(tx, actor, peer).await?;

    if task["state"] != "prepared" || task["claim_id"].as_str() != Some(claim_id) {
        return Err(CoreError::conflict(
            "peer task is not prepared under this claim",
        ));
    }
    let prepare = sqlx::query(
        "SELECT actor,claim_id,state,result FROM sic_peer_task_prepares WHERE task_id=?",
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| CoreError::conflict("prepared task has no durable prepare receipt"))?;
    let prepare_actor: String = prepare.try_get("actor")?;
    let prepare_claim: String = prepare.try_get("claim_id")?;
    let prepare_state: String = prepare.try_get("state")?;
    let encoded: Option<String> = prepare.try_get("result")?;
    let receipt: Value = serde_json::from_str(
        encoded
            .as_deref()
            .ok_or_else(|| CoreError::conflict("prepare receipt has no result"))?,
    )?;
    if prepare_actor != actor
        || prepare_claim != claim_id
        || prepare_state != "prepared"
        || receipt["ready_for_dsh"] != true
        || receipt["state"] != "prepared"
        || receipt["evidence"]["spawn_message_delivery"]["verified"] != true
        || receipt["peer_chain_id"].as_str() != Some(peer)
    {
        return Err(CoreError::conflict(
            "prepare receipt is not verified for DSH launch",
        ));
    }
    sqlx::query("INSERT INTO sic_peer_process_launches(task_id,launch_id,actor,peer_chain_id,claim_id,fingerprint,executable,state) VALUES(?,?,?,?,?,?,?,'intent')")
        .bind(task_id).bind(launch_id).bind(actor).bind(peer).bind(claim_id)
        .bind(fingerprint).bind(executable.to_string_lossy().as_ref())
        .execute(&mut **tx).await?;
    Ok(None)
}

async fn verify_chain_owner(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain_id: &str,
) -> Result<()> {
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(chain_id)
        .fetch_optional(&mut **tx)
        .await?;
    let state = state.ok_or_else(|| CoreError::new("not_found", "peer route not found", 404))?;
    let value: Value = serde_json::from_str(&state)?;
    if !value["owners"]
        .as_array()
        .is_some_and(|owners| owners.iter().any(|o| o.as_str() == Some(actor)))
    {
        return Err(CoreError::forbidden("actor does not own the peer route"));
    }
    Ok(())
}

async fn set_state(
    store: &Store,
    actor: &str,
    peer: &str,
    task: &str,
    launch: &str,
    state: &str,
    code: Option<i32>,
    reason: Option<&str>,
) -> Result<()> {
    let mut tx = store.begin().await?;
    let _ = peer_tasks::get_scoped_in_transaction(&mut tx, actor, peer, task).await?;
    verify_chain_owner(&mut tx, actor, peer).await?;
    let changed = sqlx::query("UPDATE sic_peer_process_launches SET state=?,exit_code=?,reason=?,updated_at=CURRENT_TIMESTAMP WHERE task_id=? AND launch_id=? AND actor=? AND peer_chain_id=? AND state IN ('intent','running')")
        .bind(state).bind(code).bind(reason).bind(task).bind(launch).bind(actor).bind(peer).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(CoreError::conflict(
            "process launch state changed unexpectedly",
        ));
    }
    tx.commit().await?;
    Ok(())
}

async fn load_receipt(
    store: &Store,
    actor: &str,
    peer: &str,
    task: &str,
    launch: &str,
    replayed: bool,
) -> Result<LaunchReceipt> {
    let mut tx = store.begin().await?;
    let _ = peer_tasks::get_scoped_in_transaction(&mut tx, actor, peer, task).await?;
    verify_chain_owner(&mut tx, actor, peer).await?;
    let row = sqlx::query("SELECT launch_id,actor,peer_chain_id,claim_id,fingerprint,state,exit_code FROM sic_peer_process_launches WHERE task_id=? AND launch_id=? AND actor=? AND peer_chain_id=?")
        .bind(task).bind(launch).bind(actor).bind(peer).fetch_optional(&mut *tx).await?
        .ok_or_else(|| CoreError::new("not_found", "process launch receipt not found", 404))?;
    let receipt = row_receipt(&row, task, replayed)?;
    tx.commit().await?;
    Ok(receipt)
}

fn row_receipt(row: &sqlx::sqlite::SqliteRow, task: &str, replayed: bool) -> Result<LaunchReceipt> {
    let state: String = row.try_get("state")?;
    Ok(LaunchReceipt {
        task_id: task.to_owned(),
        launch_id: row.try_get("launch_id")?,
        state: state.clone(),
        exit_code: row.try_get("exit_code")?,
        replayed,
        process_exited: state == "exited",
    })
}

fn validate_spec(spec: &HostProcessSpec) -> Result<()> {
    if !spec.executable.is_absolute() || !spec.executable.is_file() {
        return Err(CoreError::invalid(
            "host executable must be an existing absolute file",
        ));
    }
    if spec
        .working_directory
        .as_ref()
        .is_some_and(|path| !path.is_absolute() || !path.is_dir())
    {
        return Err(CoreError::invalid(
            "working directory must be an existing absolute directory",
        ));
    }
    if spec.timeout.is_zero() || spec.timeout > Duration::from_secs(3600) {
        return Err(CoreError::invalid(
            "process timeout must be between 1ms and 3600s",
        ));
    }
    if spec.argv.len() > 128
        || spec
            .argv
            .iter()
            .any(|s| s.contains('\0') || s.len() > 16_384)
    {
        return Err(CoreError::invalid("host argv exceeds supported bounds"));
    }
    if spec.environment.len() > 64
        || spec.environment.iter().any(|(k, v)| {
            k.is_empty()
                || k.contains('=')
                || k.contains('\0')
                || v.contains('\0')
                || k.len() > 256
                || v.len() > 32_768
        })
    {
        return Err(CoreError::invalid("host environment allowlist is invalid"));
    }
    Ok(())
}

fn validate_program(path: &Path) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = path
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "cmd" | "bat")
        || matches!(stem.as_str(), "cmd" | "powershell" | "pwsh")
    {
        return Err(CoreError::invalid(
            "shell executable or script launch is forbidden",
        ));
    }
    Ok(())
}

fn spec_fingerprint(executable: &Path, spec: &HostProcessSpec) -> Result<String> {
    // No argument or environment contents are written to storage. Their digest
    // binds a replay to the exact host specification without logging values.
    let env: Vec<Value> = spec
        .environment
        .iter()
        .map(|(key, value)| json!({"key":key,"value_digest":storage::sha256(value.as_bytes())}))
        .collect();
    storage::digest(&json!({
        "executable":executable.to_string_lossy(), "argv":spec.argv,
        "environment":env, "working_directory":spec.working_directory,
        "timeout_ms":spec.timeout.as_millis()
    }))
}

fn validate_key(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(CoreError::invalid(format!(
            "{field} must be nonempty and at most 128 bytes"
        )));
    }
    Ok(())
}
