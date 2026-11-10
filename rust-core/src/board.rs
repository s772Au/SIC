//! Transactional shared-board state for cross-agent notices and task routing.
//!
//! ACL configuration is host-owned. Mutations append an audit event and an
//! idempotency receipt while advancing a board-wide revision with CAS. The
//! latest I view is materialized on read, so a Core publication can fetch it
//! inside the same SQLite transaction that stores its CB.

use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    error::{CoreError, Result},
    storage::Store,
};

const TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sic_boards (
        board_id TEXT PRIMARY KEY NOT NULL,
        owner_actor TEXT NOT NULL,
        revision INTEGER NOT NULL,
        acl_json TEXT NOT NULL,
        state_json TEXT NOT NULL,
        state_digest TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    )",
    "CREATE TABLE IF NOT EXISTS sic_board_events (
        event_id TEXT PRIMARY KEY NOT NULL,
        board_id TEXT NOT NULL,
        revision INTEGER NOT NULL,
        actor TEXT NOT NULL,
        action TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        result_json TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE(board_id, revision)
    )",
    "CREATE TRIGGER IF NOT EXISTS sic_board_events_no_update BEFORE UPDATE ON sic_board_events BEGIN SELECT RAISE(ABORT,'board events are immutable'); END",
    "CREATE TRIGGER IF NOT EXISTS sic_board_events_no_delete BEFORE DELETE ON sic_board_events BEGIN SELECT RAISE(ABORT,'board events are immutable'); END",
    "CREATE TABLE IF NOT EXISTS sic_board_receipts (
        board_id TEXT NOT NULL,
        actor TEXT NOT NULL,
        idempotency_key TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        result_json TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY(board_id, actor, idempotency_key)
    )",
    "CREATE TRIGGER IF NOT EXISTS sic_board_receipts_no_update BEFORE UPDATE ON sic_board_receipts BEGIN SELECT RAISE(ABORT,'board receipts are immutable'); END",
    "CREATE TRIGGER IF NOT EXISTS sic_board_receipts_no_delete BEFORE DELETE ON sic_board_receipts BEGIN SELECT RAISE(ABORT,'board receipts are immutable'); END",
];

/// Created by trusted host code from an authenticated operation context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardActor(String);

impl BoardActor {
    pub fn from_authenticated_host(actor: impl Into<String>) -> Result<Self> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(CoreError::forbidden(
                "authenticated board actor is required",
            ));
        }
        Ok(Self(actor))
    }
    pub fn actor(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardAcl {
    readers: BTreeSet<String>,
    writers: BTreeSet<String>,
    claimers: BTreeSet<String>,
    managers: BTreeSet<String>,
}

impl BoardAcl {
    pub fn new(
        readers: impl IntoIterator<Item = impl Into<String>>,
        writers: impl IntoIterator<Item = impl Into<String>>,
        claimers: impl IntoIterator<Item = impl Into<String>>,
        managers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self> {
        let acl = Self {
            readers: collect_principals(readers)?,
            writers: collect_principals(writers)?,
            claimers: collect_principals(claimers)?,
            managers: collect_principals(managers)?,
        };
        Ok(acl)
    }
    pub fn readers(&self) -> &BTreeSet<String> {
        &self.readers
    }
    pub fn writers(&self) -> &BTreeSet<String> {
        &self.writers
    }
    pub fn claimers(&self) -> &BTreeSet<String> {
        &self.claimers
    }
    pub fn managers(&self) -> &BTreeSet<String> {
        &self.managers
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoardEntryKind {
    Text,
    Structured,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BoardEntry {
    pub entry_id: String,
    pub actor: String,
    pub kind: BoardEntryKind,
    pub content: Value,
    pub revision: u64,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    Claimed,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BoardTask {
    pub task_id: String,
    pub creator_actor: String,
    pub assignee_actor: Option<String>,
    pub title: String,
    pub detail: Value,
    pub status: TaskStatus,
    pub revision: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct BoardState {
    entries: Vec<BoardEntry>,
    tasks: Vec<BoardTask>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BoardSnapshot {
    pub board_id: String,
    pub revision: u64,
    pub entries: Vec<BoardEntry>,
    pub tasks: Vec<BoardTask>,
    pub latest_i: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BoardEvent {
    pub event_id: String,
    pub board_id: String,
    pub revision: u64,
    pub actor: String,
    pub action: String,
    pub payload: Value,
    pub result: Value,
    pub created_at: String,
}

#[derive(Clone)]
pub struct BoardService {
    store: Store,
}

impl BoardService {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn initialize(&self) -> Result<()> {
        let mut tx = self.store.begin().await?;
        Self::initialize_in_transaction(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn initialize_in_transaction(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        for sql in TABLES {
            sqlx::query(sql).execute(&mut **tx).await?;
        }
        Ok(())
    }

    /// Initial ACL and board creation are host provisioning actions.
    pub async fn configure_from_host(
        &self,
        board_id: &str,
        actor: &BoardActor,
        acl: BoardAcl,
    ) -> Result<BoardSnapshot> {
        let mut tx = self.store.begin().await?;
        let result =
            Self::configure_from_host_in_transaction(&mut tx, board_id, actor, acl).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn configure_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        board_id: &str,
        actor: &BoardActor,
        acl: BoardAcl,
    ) -> Result<BoardSnapshot> {
        validate_id(board_id, "board_id")?;
        Self::initialize_in_transaction(tx).await?;
        let state = BoardState {
            entries: Vec::new(),
            tasks: Vec::new(),
        };
        let state_json = serde_json::to_string(&state)
            .map_err(|_| CoreError::invalid("board state is not serializable"))?;
        let acl_json = serde_json::to_string(&acl)
            .map_err(|_| CoreError::invalid("board ACL is not serializable"))?;
        let digest = crate::storage::sha256(state_json.as_bytes());
        sqlx::query("INSERT INTO sic_boards(board_id,owner_actor,revision,acl_json,state_json,state_digest) VALUES(?,?,1,?,?,?)")
            .bind(board_id).bind(actor.actor()).bind(&acl_json).bind(&state_json).bind(digest).execute(&mut **tx).await
            .map_err(|error| if is_unique(&error) { CoreError::conflict("board already exists") } else { CoreError::storage() })?;
        let view = materialize_i(board_id, 1, &state)?;
        let result = BoardSnapshot {
            board_id: board_id.into(),
            revision: 1,
            entries: state.entries,
            tasks: state.tasks,
            latest_i: view,
        };
        let result_json = serde_json::to_value(&result)
            .map_err(|_| CoreError::invalid("board snapshot is not serializable"))?;
        let event_id = crate::storage::uid("board_event");
        sqlx::query("INSERT INTO sic_board_events(event_id,board_id,revision,actor,action,payload_json,result_json) VALUES(?,?,1,?,'configure',?,?)")
            .bind(event_id).bind(board_id).bind(actor.actor()).bind(acl_json).bind(serde_json::to_string(&result_json).map_err(|_| CoreError::invalid("board event is invalid"))?)
            .execute(&mut **tx).await?;
        Ok(result)
    }

    /// Replace grants only through trusted host code. The current manager ACL
    /// is checked before any revision change; the mutation is audited.
    pub async fn update_acl_from_host_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        acl: BoardAcl,
    ) -> Result<Value> {
        let acl_value = serde_json::to_value(&acl)
            .map_err(|_| CoreError::invalid("board ACL is not serializable"))?;
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "acl.update",
            &acl_value,
            Permission::Manage,
            Some(acl),
            |_state, next| Ok(json!({"board_id":board_id,"revision":next,"acl_updated":true})),
        )
        .await
    }

    pub async fn update_acl_from_host(
        &self,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        acl: BoardAcl,
    ) -> Result<Value> {
        let mut tx = self.store.begin().await?;
        let result = Self::update_acl_from_host_in_transaction(
            &mut tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            acl,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn publish_text_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        text: &str,
    ) -> Result<Value> {
        if text.trim().is_empty() {
            return Err(CoreError::invalid("board text entry must not be empty"));
        }
        let payload = json!({"text":text});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "entry.publish_text",
            &payload,
            Permission::Write,
            None,
            |state, next| {
                let entry = BoardEntry {
                    entry_id: crate::storage::uid("board_entry"),
                    actor: actor.actor().into(),
                    kind: BoardEntryKind::Text,
                    content: json!(text),
                    revision: next,
                    created_at: now(),
                };
                state.entries.push(entry.clone());
                Ok(json!({"board_id":board_id,"revision":next,"entry":entry}))
            },
        )
        .await
    }

    pub async fn publish_structured_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        content: &Value,
    ) -> Result<Value> {
        let payload = json!({"content":content});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "entry.publish_structured",
            &payload,
            Permission::Write,
            None,
            |state, next| {
                let entry = BoardEntry {
                    entry_id: crate::storage::uid("board_entry"),
                    actor: actor.actor().into(),
                    kind: BoardEntryKind::Structured,
                    content: content.clone(),
                    revision: next,
                    created_at: now(),
                };
                state.entries.push(entry.clone());
                Ok(json!({"board_id":board_id,"revision":next,"entry":entry}))
            },
        )
        .await
    }

    pub async fn create_task_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        title: &str,
        detail: &Value,
    ) -> Result<Value> {
        if title.trim().is_empty() {
            return Err(CoreError::invalid("task title must not be empty"));
        }
        let payload = json!({"title":title,"detail":detail});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "task.create",
            &payload,
            Permission::Write,
            None,
            |state, next| {
                let task = BoardTask {
                    task_id: crate::storage::uid("board_task"),
                    creator_actor: actor.actor().into(),
                    assignee_actor: None,
                    title: title.into(),
                    detail: detail.clone(),
                    status: TaskStatus::Open,
                    revision: next,
                    updated_at: now(),
                };
                state.tasks.push(task.clone());
                Ok(json!({"board_id":board_id,"revision":next,"task":task}))
            },
        )
        .await
    }

    pub async fn claim_task_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        task_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
    ) -> Result<Value> {
        let payload = json!({"task_id":task_id});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "task.claim",
            &payload,
            Permission::Claim,
            None,
            |state, next| {
                let task = find_task_mut(state, task_id)?;
                if task.status != TaskStatus::Open || task.assignee_actor.is_some() {
                    return Err(CoreError::conflict("task is already claimed or not open"));
                }
                task.status = TaskStatus::Claimed;
                task.assignee_actor = Some(actor.actor().into());
                task.revision = next;
                task.updated_at = now();
                Ok(json!({"board_id":board_id,"revision":next,"task":task}))
            },
        )
        .await
    }

    pub async fn release_task_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        task_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
    ) -> Result<Value> {
        let permissions = load_board(tx, board_id).await?;
        let may_manage =
            permissions.owner == actor.actor() || permissions.acl.managers.contains(actor.actor());
        let payload = json!({"task_id":task_id});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "task.release",
            &payload,
            Permission::Release,
            None,
            |state, next| {
                let task = find_task_mut(state, task_id)?;
                if task.assignee_actor.as_deref() != Some(actor.actor()) && !may_manage {
                    return Err(CoreError::forbidden(
                        "only the task claimant or board manager can release it",
                    ));
                }
                if !matches!(
                    task.status,
                    TaskStatus::Claimed | TaskStatus::InProgress | TaskStatus::Blocked
                ) {
                    return Err(CoreError::conflict(
                        "task cannot be released from its current status",
                    ));
                }
                task.status = TaskStatus::Open;
                task.assignee_actor = None;
                task.revision = next;
                task.updated_at = now();
                Ok(json!({"board_id":board_id,"revision":next,"task":task}))
            },
        )
        .await
    }

    pub async fn transition_task_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        task_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        next_status: TaskStatus,
    ) -> Result<Value> {
        let payload = json!({"task_id":task_id,"status":next_status});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "task.transition",
            &payload,
            Permission::Manage,
            None,
            |state, next| {
                let task = find_task_mut(state, task_id)?;
                if !valid_transition(task.status, next_status) {
                    return Err(CoreError::conflict("invalid task status transition"));
                }
                if next_status == TaskStatus::Open {
                    task.assignee_actor = None;
                }
                task.status = next_status;
                task.revision = next;
                task.updated_at = now();
                Ok(json!({"board_id":board_id,"revision":next,"task":task}))
            },
        )
        .await
    }

    /// A claimant can report progress or a blocker on its own task. Completion
    /// and cancellation remain separately controlled by board management.
    pub async fn update_own_task_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        task_id: &str,
        expected_revision: u64,
        idempotency_key: &str,
        status: TaskStatus,
    ) -> Result<Value> {
        if !matches!(status, TaskStatus::InProgress | TaskStatus::Blocked) {
            return Err(CoreError::forbidden(
                "claimants may only report in_progress or blocked",
            ));
        }
        let payload = json!({"task_id":task_id,"status":status});
        apply_mutation(
            tx,
            actor,
            board_id,
            expected_revision,
            idempotency_key,
            "task.progress",
            &payload,
            Permission::Claim,
            None,
            |state, next| {
                let task = find_task_mut(state, task_id)?;
                if task.assignee_actor.as_deref() != Some(actor.actor()) {
                    return Err(CoreError::forbidden(
                        "only the current task claimant can report progress",
                    ));
                }
                if !valid_transition(task.status, status) {
                    return Err(CoreError::conflict("invalid task status transition"));
                }
                task.status = status;
                task.revision = next;
                task.updated_at = now();
                Ok(json!({"board_id":board_id,"revision":next,"task":task}))
            },
        )
        .await
    }

    pub async fn read_latest_i_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
    ) -> Result<Value> {
        let loaded = load_board(tx, board_id).await?;
        require(&loaded.acl, actor.actor(), Permission::Read)?;
        materialize_i(board_id, loaded.revision, &loaded.state)
    }

    /// Publication helper: supplying the I revision observed during the edit
    /// lets a caller reject stale material before inserting a new CB.
    pub async fn read_latest_i_for_publish_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        expected_revision: Option<u64>,
    ) -> Result<Value> {
        let i = Self::read_latest_i_in_transaction(tx, actor, board_id).await?;
        if expected_revision.is_some_and(|expected| i["revision"].as_u64() != Some(expected)) {
            return Err(CoreError::conflict(
                "shared board I changed since the Editor snapshot",
            ));
        }
        Ok(i)
    }

    pub async fn read_history_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        actor: &BoardActor,
        board_id: &str,
        after_revision: u64,
        limit: u64,
    ) -> Result<Vec<BoardEvent>> {
        if limit == 0 || limit > 500 {
            return Err(CoreError::invalid("history limit must be 1..500"));
        }
        if after_revision > i64::MAX as u64 {
            return Err(CoreError::invalid(
                "history revision is outside the supported range",
            ));
        }
        let loaded = load_board(tx, board_id).await?;
        require(&loaded.acl, actor.actor(), Permission::Read)?;
        let rows = sqlx::query("SELECT event_id,revision,actor,action,payload_json,result_json,created_at FROM sic_board_events WHERE board_id=? AND revision>? ORDER BY revision LIMIT ?")
            .bind(board_id).bind(after_revision as i64).bind(limit as i64).fetch_all(&mut **tx).await?;
        rows.into_iter()
            .map(|row| {
                let payload: String = row.try_get("payload_json")?;
                let result: String = row.try_get("result_json")?;
                Ok(BoardEvent {
                    event_id: row.try_get("event_id")?,
                    board_id: board_id.into(),
                    revision: row.try_get::<i64, _>("revision")? as u64,
                    actor: row.try_get("actor")?,
                    action: row.try_get("action")?,
                    payload: serde_json::from_str(&payload).map_err(|_| {
                        CoreError::new("board_event_invalid", "stored board event is invalid", 500)
                    })?,
                    result: serde_json::from_str(&result).map_err(|_| {
                        CoreError::new("board_event_invalid", "stored board result is invalid", 500)
                    })?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Permission {
    Read,
    Write,
    Claim,
    Manage,
    Release,
}

struct LoadedBoard {
    owner: String,
    revision: u64,
    acl: BoardAcl,
    state: BoardState,
}

async fn apply_mutation<F>(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &BoardActor,
    board_id: &str,
    expected_revision: u64,
    idempotency_key: &str,
    action: &str,
    payload: &Value,
    permission: Permission,
    acl_override: Option<BoardAcl>,
    mutate: F,
) -> Result<Value>
where
    F: FnOnce(&mut BoardState, u64) -> Result<Value>,
{
    validate_id(board_id, "board_id")?;
    validate_id(idempotency_key, "idempotency_key")?;
    BoardService::initialize_in_transaction(tx).await?;
    let mut loaded = load_board(tx, board_id).await?;
    if !(matches!(permission, Permission::Manage) && loaded.owner == actor.actor()) {
        require(&loaded.acl, actor.actor(), permission)?;
    }
    let fingerprint = crate::storage::digest(
        &json!({"action":action,"payload":payload,"expected_revision":expected_revision}),
    )?;
    if let Some(row)=sqlx::query("SELECT fingerprint,result_json FROM sic_board_receipts WHERE board_id=? AND actor=? AND idempotency_key=?")
        .bind(board_id).bind(actor.actor()).bind(idempotency_key).fetch_optional(&mut **tx).await? {
        let prior:String=row.try_get("fingerprint")?;
        if prior!=fingerprint { return Err(CoreError::conflict("board idempotency key reused with different request")); }
        let result:String=row.try_get("result_json")?;
        return serde_json::from_str(&result).map_err(|_|CoreError::new("board_receipt_invalid","stored board receipt is invalid",500));
    }
    if expected_revision > i64::MAX as u64 {
        return Err(CoreError::invalid(
            "board revision is outside the supported range",
        ));
    }
    if loaded.revision != expected_revision {
        return Err(CoreError::conflict("board revision changed"));
    }
    if loaded.revision >= i64::MAX as u64 {
        return Err(CoreError::conflict("board revision exhausted"));
    }
    let next = loaded
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::conflict("board revision exhausted"))?;
    let result = mutate(&mut loaded.state, next)?;
    let acl = acl_override.unwrap_or(loaded.acl);
    let state_json = serde_json::to_string(&loaded.state)
        .map_err(|_| CoreError::invalid("board state is not serializable"))?;
    let state_digest = crate::storage::sha256(state_json.as_bytes());
    let acl_json = serde_json::to_string(&acl)
        .map_err(|_| CoreError::invalid("board ACL is not serializable"))?;
    let updated=sqlx::query("UPDATE sic_boards SET revision=?,acl_json=?,state_json=?,state_digest=? WHERE board_id=? AND revision=?")
        .bind(next as i64).bind(&acl_json).bind(&state_json).bind(state_digest).bind(board_id).bind(expected_revision as i64)
        .execute(&mut **tx).await?.rows_affected();
    if updated != 1 {
        return Err(CoreError::conflict("board revision changed"));
    }
    let event_id = crate::storage::uid("board_event");
    let payload_json = serde_json::to_string(payload)
        .map_err(|_| CoreError::invalid("board event payload is invalid"))?;
    let result_json = serde_json::to_string(&result)
        .map_err(|_| CoreError::invalid("board result is invalid"))?;
    sqlx::query("INSERT INTO sic_board_events(event_id,board_id,revision,actor,action,payload_json,result_json) VALUES(?,?,?,?,?,?,?)")
        .bind(&event_id).bind(board_id).bind(next as i64).bind(actor.actor()).bind(action).bind(payload_json).bind(&result_json).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO sic_board_receipts(board_id,actor,idempotency_key,fingerprint,result_json) VALUES(?,?,?,?,?)")
        .bind(board_id).bind(actor.actor()).bind(idempotency_key).bind(fingerprint).bind(result_json).execute(&mut **tx).await?;
    Ok(result)
}

async fn load_board(tx: &mut Transaction<'_, Sqlite>, board_id: &str) -> Result<LoadedBoard> {
    BoardService::initialize_in_transaction(tx).await?;
    let row=sqlx::query("SELECT owner_actor,revision,acl_json,state_json,state_digest FROM sic_boards WHERE board_id=?")
        .bind(board_id).fetch_optional(&mut **tx).await?.ok_or_else(||CoreError::new("not_found","board not found",404))?;
    let state_json: String = row.try_get("state_json")?;
    let digest: String = row.try_get("state_digest")?;
    if crate::storage::sha256(state_json.as_bytes()) != digest {
        return Err(CoreError::new(
            "integrity_error",
            "board state integrity check failed",
            500,
        ));
    }
    let acl_json: String = row.try_get("acl_json")?;
    let state: BoardState = serde_json::from_str(&state_json)
        .map_err(|_| CoreError::new("board_state_invalid", "stored board state is invalid", 500))?;
    let acl: BoardAcl = serde_json::from_str(&acl_json)
        .map_err(|_| CoreError::new("board_acl_invalid", "stored board ACL is invalid", 500))?;
    let revision: i64 = row.try_get("revision")?;
    if revision < 0 {
        return Err(CoreError::new(
            "integrity_error",
            "board revision is invalid",
            500,
        ));
    }
    Ok(LoadedBoard {
        owner: row.try_get("owner_actor")?,
        revision: revision as u64,
        acl,
        state,
    })
}

fn require(acl: &BoardAcl, actor: &str, permission: Permission) -> Result<()> {
    let allowed = match permission {
        Permission::Read => acl.readers.contains(actor),
        Permission::Write => acl.writers.contains(actor),
        Permission::Claim => acl.claimers.contains(actor),
        Permission::Manage => acl.managers.contains(actor),
        Permission::Release => acl.managers.contains(actor) || acl.claimers.contains(actor),
    };
    if allowed {
        Ok(())
    } else {
        Err(CoreError::forbidden("board permission denied"))
    }
}

fn find_task_mut<'a>(state: &'a mut BoardState, task_id: &str) -> Result<&'a mut BoardTask> {
    state
        .tasks
        .iter_mut()
        .find(|task| task.task_id == task_id)
        .ok_or_else(|| CoreError::new("not_found", "board task not found", 404))
}

fn valid_transition(current: TaskStatus, next: TaskStatus) -> bool {
    matches!(
        (current, next),
        (TaskStatus::Open, TaskStatus::Cancelled)
            | (
                TaskStatus::Claimed,
                TaskStatus::InProgress | TaskStatus::Blocked | TaskStatus::Cancelled
            )
            | (
                TaskStatus::InProgress,
                TaskStatus::Blocked | TaskStatus::Done | TaskStatus::Cancelled
            )
            | (
                TaskStatus::Blocked,
                TaskStatus::InProgress | TaskStatus::Cancelled
            )
    )
}

fn materialize_i(board_id: &str, revision: u64, state: &BoardState) -> Result<Value> {
    let mut view = json!({"board_id":board_id,"revision":revision,"entries":state.entries,"tasks":state.tasks});
    let digest = crate::storage::digest(&view)?;
    view["digest"] = json!(digest);
    Ok(view)
}

fn collect_principals(
    values: impl IntoIterator<Item = impl Into<String>>,
) -> Result<BTreeSet<String>> {
    values
        .into_iter()
        .map(|value| {
            let value = value.into();
            if value.trim().is_empty() {
                Err(CoreError::invalid("board ACL actor must not be empty"))
            } else {
                Ok(value)
            }
        })
        .collect()
}
fn validate_id(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CoreError::invalid(format!("{name} must not be empty")))
    } else {
        Ok(())
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn is_unique(error: &sqlx::Error) -> bool {
    matches!(error,sqlx::Error::Database(database) if database.is_unique_violation())
}
