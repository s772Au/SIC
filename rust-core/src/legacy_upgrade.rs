//! Safe upgrade path for the active Python database shape that has no source,
//! CB, or job objects but has Board, dynamic tools, and runtime queue rows.
//! All source reads come from one read-only, digest-verified snapshot.

use std::{collections::BTreeSet, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{
    board::{BoardAcl, BoardActor, BoardService},
    core::{Core, OperationContext},
    error::{CoreError, Result},
    legacy::{LegacyRawSnapshot, RawCell, RawTable, snapshot_python_records},
    storage::{self, Store},
    tool_store::ToolStore,
    toolset::{ToolAcl, ToolDescriptor},
};

const BASE_COLUMNS: &[(&str, &[&str])] = &[
    ("objects", &["id", "kind", "data", "digest"]),
    ("chains", &["id", "state"]),
    ("boards", &["id", "state"]),
    ("revocations", &["id", "record"]),
    ("receipts", &["key", "fingerprint", "result"]),
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpgradeReport {
    pub schema: String,
    pub target_chain_id: String,
    pub original_chain_id: String,
    pub original_owner_count: usize,
    pub imported_board_count: usize,
    pub quarantined_tool_count: usize,
    pub quarantined_runtime_event_count: usize,
    pub pending_or_running_runtime_event_count: usize,
    pub archived_bridge_binding_count: usize,
    pub archived_bridge_turn_count: usize,
    pub archived_host_tool_host_count: usize,
    pub archived_host_tool_task_count: usize,
    pub source_archive_object_id: String,
    pub source_archive_sha256: String,
    pub migration_manifest_object_id: String,
    pub source_database_modified: bool,
    pub target_agent_run_state: String,
    pub executable_legacy_work_resumed: bool,
}

struct LegacyTool {
    name: String,
    description: String,
    schema: Value,
    discover_acl: Vec<String>,
    expand_acl: Vec<String>,
    execute_acl: Vec<String>,
    expanded: bool,
}

struct LegacyBoard {
    id: String,
    state: Value,
}

struct QueueRow {
    id: String,
    chain_id: Option<String>,
    actor: Option<String>,
    event: Option<String>,
    status: Option<String>,
    raw: Value,
}

#[derive(Default)]
struct SidecarCounts {
    bridge_bindings: usize,
    bridge_turns: usize,
    host_tool_hosts: usize,
    host_tool_tasks: usize,
}

struct Prepared {
    snapshot: LegacyRawSnapshot,
    original_chain_id: String,
    original_chain_state: Value,
    original_chain_state_sha256: String,
    owners: Vec<String>,
    owner_actor: String,
    tools: Vec<LegacyTool>,
    boards: Vec<LegacyBoard>,
    queue: Vec<QueueRow>,
    sidecars: SidecarCounts,
}

fn refused(message: impl Into<String>) -> CoreError {
    CoreError::new("legacy_upgrade_refused", message, 409)
}

fn table<'a>(snapshot: &'a LegacyRawSnapshot, name: &str) -> Option<&'a RawTable> {
    snapshot.tables.iter().find(|item| item.name == name)
}

fn column<'a>(row: &'a [RawCell], columns: &[String], name: &str) -> Result<&'a RawCell> {
    let index = columns
        .iter()
        .position(|column| column == name)
        .ok_or_else(|| refused(format!("legacy column {name} is missing")))?;
    row.get(index)
        .ok_or_else(|| refused("legacy raw row width does not match schema"))
}

fn hex_bytes(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(refused("legacy raw cell has odd-length hex"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            let high = digit(pair[0]).ok_or_else(|| refused("legacy raw cell hex is malformed"))?;
            let low = digit(pair[1]).ok_or_else(|| refused("legacy raw cell hex is malformed"))?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn text(row: &[RawCell], columns: &[String], name: &str) -> Result<String> {
    let value = column(row, columns, name)?;
    if value.storage_class != "TEXT" {
        return Err(refused(format!("legacy {name} must be TEXT")));
    }
    let bytes = value
        .value
        .get("hex")
        .and_then(Value::as_str)
        .ok_or_else(|| refused(format!("legacy {name} raw bytes are unavailable")))?;
    String::from_utf8(hex_bytes(bytes)?)
        .map_err(|_| refused(format!("legacy {name} is not valid UTF-8")))
}

fn optional_text(row: &[RawCell], columns: &[String], name: &str) -> Result<Option<String>> {
    if column(row, columns, name)?.storage_class == "NULL" {
        return Ok(None);
    }
    text(row, columns, name).map(Some)
}

fn string_list(value: &Value, key: &str) -> Result<Vec<String>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| refused(format!("legacy {key} must be an array")))?
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|identity| !identity.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| refused(format!("legacy {key} contains an invalid identity")))
        })
        .collect()
}

fn validate_snapshot(snapshot: &LegacyRawSnapshot) -> Result<()> {
    snapshot.verify()?;
    for (name, expected) in BASE_COLUMNS {
        let item = table(snapshot, name)
            .ok_or_else(|| refused(format!("legacy table {name} is missing")))?;
        if item.columns.iter().map(String::as_str).collect::<Vec<_>>() != *expected {
            return Err(refused(format!(
                "legacy table {name} has an unsupported shape"
            )));
        }
    }
    for item in &snapshot.tables {
        if item.rows.len() != item.row_sha256.len() || item.rows.len() != item.row_ids.len() {
            return Err(refused(format!(
                "legacy {} row digest count is inconsistent",
                item.name
            )));
        }
        for ((row, rowid), expected) in item.rows.iter().zip(&item.row_ids).zip(&item.row_sha256) {
            let encoded = serde_json::to_vec(&(rowid, row))
                .map_err(|_| refused("legacy row digest input cannot be serialized"))?;
            if storage::sha256(&encoded) != *expected {
                return Err(refused(format!("legacy {} row digest mismatch", item.name)));
            }
        }
    }
    Ok(())
}

fn decode_tools(state: &Value) -> Result<Vec<LegacyTool>> {
    let rows = state
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("legacy Agent tools must be an array"))?;
    let mut names = BTreeSet::new();
    rows.iter()
        .map(|row| {
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| refused("legacy tool has no valid name"))?
                .to_owned();
            if !names.insert(name.clone()) {
                return Err(refused("legacy tool names are not unique"));
            }
            let description = row
                .get("descriptor")
                .and_then(Value::as_str)
                .ok_or_else(|| refused(format!("legacy tool {name} has no description")))?
                .to_owned();
            let schema = row
                .get("schema")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or_else(|| refused(format!("legacy tool {name} has no schema")))?;
            if schema.pointer("/function/name").and_then(Value::as_str) != Some(&name) {
                return Err(refused(format!(
                    "legacy tool {name} schema identity differs"
                )));
            }
            let expanded = row
                .get("expanded")
                .and_then(Value::as_bool)
                .ok_or_else(|| refused(format!("legacy tool {name} expanded flag is invalid")))?;
            Ok(LegacyTool {
                name,
                description,
                schema,
                discover_acl: string_list(row, "discover_acl")?,
                expand_acl: string_list(row, "expand_acl")?,
                execute_acl: string_list(row, "execute_acl")?,
                expanded,
            })
        })
        .collect()
}

fn decode_boards(snapshot: &LegacyRawSnapshot) -> Result<Vec<LegacyBoard>> {
    let Some(item) = table(snapshot, "boards") else {
        return Ok(Vec::new());
    };
    item.rows
        .iter()
        .map(|row| {
            let id = text(row, &item.columns, "id")?;
            let raw_state = text(row, &item.columns, "state")?;
            let state: Value = serde_json::from_str(&raw_state)
                .map_err(|_| refused(format!("legacy Board {id} state is malformed")))?;
            if !state.is_object()
                || state.get("value").is_none()
                || state.get("revision").and_then(Value::as_u64).is_none()
                || state.get("owner").and_then(Value::as_str).is_none()
            {
                return Err(refused(format!(
                    "legacy Board {id} lacks owner, revision, or value"
                )));
            }
            let _ = string_list(&state, "read_acl")?;
            let _ = string_list(&state, "write_acl")?;
            Ok(LegacyBoard { id, state })
        })
        .collect()
}

fn raw_row(item: &RawTable, index: usize) -> Value {
    let fields = item
        .columns
        .iter()
        .zip(&item.rows[index])
        .map(|(name, cell)| {
            (
                name.clone(),
                json!({"storage_class":cell.storage_class,"value":cell.value}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "table":item.name,
        "rowid":item.row_ids[index],
        "row_sha256":item.row_sha256[index],
        "fields":fields
    })
}

fn decode_queue(snapshot: &LegacyRawSnapshot) -> Result<Vec<QueueRow>> {
    let Some(item) = table(snapshot, "runtime_queue") else {
        return Ok(Vec::new());
    };
    let expected = ["id", "chain_id", "actor", "event", "status"];
    if item.columns.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err(refused("legacy runtime_queue has an unsupported shape"));
    }
    item.rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            Ok(QueueRow {
                id: text(row, &item.columns, "id")?,
                chain_id: optional_text(row, &item.columns, "chain_id")?,
                actor: optional_text(row, &item.columns, "actor")?,
                event: optional_text(row, &item.columns, "event")?,
                status: optional_text(row, &item.columns, "status")?,
                raw: raw_row(item, index),
            })
        })
        .collect()
}

fn count(snapshot: &LegacyRawSnapshot, name: &str) -> usize {
    table(snapshot, name).map_or(0, |item| item.rows.len())
}

fn prepare(snapshot: LegacyRawSnapshot) -> Result<Prepared> {
    validate_snapshot(&snapshot)?;
    let objects = table(&snapshot, "objects").unwrap();
    if !objects.rows.is_empty() {
        return Err(refused(
            "upgrade scope requires an empty legacy object table; populated histories need a separate verified converter",
        ));
    }
    let chains = table(&snapshot, "chains").unwrap();
    if chains.rows.len() != 1 {
        return Err(refused("upgrade requires exactly one legacy Agent"));
    }
    let row = &chains.rows[0];
    let original_chain_id = text(row, &chains.columns, "id")?;
    let raw_state = text(row, &chains.columns, "state")?;
    let state: Value = serde_json::from_str(&raw_state)
        .map_err(|_| refused("legacy Agent state is malformed JSON"))?;
    if !state.is_object()
        || state.get("chain_id").and_then(Value::as_str) != Some(&original_chain_id)
    {
        return Err(refused(
            "legacy Agent chain identity does not match its row",
        ));
    }
    let owners = string_list(&state, "owners")?;
    let owner_actor = owners
        .first()
        .cloned()
        .ok_or_else(|| refused("legacy Agent has no owner"))?;
    if state.get("run_state").and_then(Value::as_str) != Some("running") {
        return Err(refused("upgrade scope accepts one running Python Agent"));
    }
    for (field, expected_empty) in [
        ("cbs", "array"),
        ("pending", "array"),
        ("groups", "object"),
        ("jobs", "object"),
    ] {
        let empty = match (expected_empty, state.get(field)) {
            ("array", Some(Value::Array(items))) => items.is_empty(),
            ("object", Some(Value::Object(items))) => items.is_empty(),
            _ => false,
        };
        if !empty {
            return Err(refused(format!(
                "legacy Agent {field} is outside upgrade scope"
            )));
        }
    }
    if state
        .get("current_cut")
        .is_some_and(|value| !value.is_null())
    {
        return Err(refused("legacy Agent current cut is outside upgrade scope"));
    }
    let tools = decode_tools(&state)?;
    let boards = decode_boards(&snapshot)?;
    if boards.len() > 1 {
        return Err(refused("upgrade supports at most one Board per Agent"));
    }
    match (
        state.get("board_id").and_then(Value::as_str),
        boards.first(),
    ) {
        (Some(source_board_id), Some(board)) if source_board_id == board.id => {
            if state.get("board").is_some_and(|embedded| {
                ["revision", "value", "owner", "read_acl", "write_acl"]
                    .iter()
                    .any(|key| embedded.get(key) != board.state.get(key))
            }) {
                return Err(refused(
                    "legacy Agent Board mirror differs from the Board table",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(refused(
                "legacy Agent Board reference does not match Board records",
            ));
        }
    }
    let queue = decode_queue(&snapshot)?;
    let sidecars = SidecarCounts {
        bridge_bindings: count(&snapshot, "bridge_bindings"),
        bridge_turns: count(&snapshot, "bridge_turns"),
        host_tool_hosts: count(&snapshot, "host_tool_hosts"),
        host_tool_tasks: count(&snapshot, "host_tool_tasks"),
    };
    Ok(Prepared {
        original_chain_state_sha256: storage::sha256(raw_state.as_bytes()),
        snapshot,
        original_chain_id,
        original_chain_state: state,
        owners,
        owner_actor,
        tools,
        boards,
        queue,
        sidecars,
    })
}

fn archive_data(snapshot: &LegacyRawSnapshot) -> Result<(String, String, String)> {
    let value = serde_json::to_value(snapshot)
        .map_err(|_| CoreError::invalid("legacy archive serialization failed"))?;
    let data = storage::json_text(&value)?;
    let digest = storage::sha256(data.as_bytes());
    Ok((storage::uid("legacy_archive"), data, digest))
}

fn tool_manifest(tools: &[LegacyTool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "name":tool.name,
                    "description_sha256":storage::sha256(tool.description.as_bytes()),
                    "schema_sha256":storage::sha256(&serde_json::to_vec(&tool.schema).unwrap_or_default()),
                    "original_acl":{"discover":tool.discover_acl,"expand":tool.expand_acl,"execute":tool.execute_acl},
                    "originally_expanded":tool.expanded,
                    "target_policy":{"discover":[],"expand":[],"execute":[],"expanded":false},
                    "disposition":"descriptor retained in isolated ToolStore; all live grants disabled"
                })
            })
            .collect(),
    )
}

/// Upgrade one active Agent with no source/CB/job objects into a fresh,
/// stopped Rust Agent. Board values become inert structured entries, original
/// actor ACLs are archived and narrowed to read-only access for the target
/// chain identity, ToolStore descriptors are retained with empty live grants,
/// runtime queue rows receive a non-runnable quarantine status, and session/
/// host-task records remain only in the raw archive.
pub async fn upgrade_active_python_agent(
    source_path: &Path,
    destination_path: &Path,
) -> Result<UpgradeReport> {
    if !source_path.is_file() {
        return Err(CoreError::invalid("legacy source database does not exist"));
    }
    if destination_path.exists() {
        return Err(CoreError::conflict("upgrade destination already exists"));
    }
    let source_path = source_path
        .canonicalize()
        .map_err(|_| CoreError::invalid("cannot resolve legacy source path"))?;
    if let Ok(destination) = destination_path.canonicalize() {
        if destination == source_path {
            return Err(CoreError::conflict(
                "source cannot be the upgrade destination",
            ));
        }
    }

    // One read-only SQLite snapshot supplies all validation and conversions.
    let prepared = prepare(snapshot_python_records(&source_path).await?)?;
    let (archive_id, archive_json, archive_digest) = archive_data(&prepared.snapshot)?;
    if let Some(parent) = destination_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|_| CoreError::invalid("cannot create destination directory"))?;
    }
    let store = Store::create(destination_path).await?;
    let core = Core::new(store.clone());
    let context = OperationContext {
        actor: prepared.owner_actor.clone(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };

    let outcome: Result<UpgradeReport> = async {
        let created = core.operate("chain.create", &context, &json!({})).await?;
        let target_chain_id = created["result"]["chain_id"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("new Rust Agent has no chain ID"))?
            .to_owned();
        let actor = BoardActor::from_authenticated_host(&target_chain_id)?;
        let mut board_mappings = Vec::new();
        let mut target_board_ids = Vec::new();
        let mut tx = store.begin().await?;

        for board in &prepared.boards {
            let target_board_id = storage::uid("legacy_board");
            let setup_acl = BoardAcl::new(
                [target_chain_id.clone()],
                [target_chain_id.clone()],
                Vec::<String>::new(),
                [target_chain_id.clone()],
            )?;
            BoardService::configure_from_host_in_transaction(
                &mut tx,
                &target_board_id,
                &actor,
                setup_acl,
            )
            .await?;
            let archived_state = json!({
                "legacy_board_id":board.id,
                "legacy_revision":board.state["revision"],
                "legacy_owner":board.state["owner"],
                "legacy_acl":{"read_acl":board.state["read_acl"],"write_acl":board.state["write_acl"]},
                "value":board.state["value"],
                "disposition":"inert structured snapshot; any embedded task status remains data and is not a live task"
            });
            BoardService::publish_structured_in_transaction(
                &mut tx,
                &actor,
                &target_board_id,
                1,
                "legacy-upgrade-value",
                &archived_state,
            )
            .await?;
            let frozen_acl = BoardAcl::new(
                [target_chain_id.clone()],
                Vec::<String>::new(),
                Vec::<String>::new(),
                Vec::<String>::new(),
            )?;
            BoardService::update_acl_from_host_in_transaction(
                &mut tx,
                &actor,
                &target_board_id,
                2,
                "legacy-upgrade-freeze-acl",
                frozen_acl,
            )
            .await?;
            board_mappings.push(json!({
                "source_board_id":board.id,
                "target_board_id":target_board_id,
                "source_revision":board.state["revision"],
                "source_acl":{"read_acl":board.state["read_acl"],"write_acl":board.state["write_acl"]},
                "target_acl":{"readers":[target_chain_id],"writers":[],"claimers":[],"managers":[]},
                "disposition":"original actor ACL is preserved in archive; live read access is narrowed to target chain identity"
            }));
            target_board_ids.push(target_board_id);
        }

        let target_tools = prepared
            .tools
            .iter()
            .map(|tool| {
                (
                    ToolDescriptor {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        schema: tool.schema.clone(),
                    },
                    false,
                )
            })
            .collect();
        ToolStore::configure_from_host_in_transaction(
            &mut tx,
            &target_chain_id,
            &prepared.owner_actor,
            ToolAcl::default(),
            target_tools,
        )
        .await?;

        let mut pending_or_running = 0usize;
        for item in &prepared.queue {
            if matches!(item.status.as_deref(), Some("pending" | "running")) {
                pending_or_running += 1;
            }
            let wrapped = json!({
                "kind":"legacy_runtime_event_quarantined",
                "original_chain_id":item.chain_id,
                "original_actor":item.actor,
                "original_event_text":item.event,
                "original_status":item.status,
                "raw_source_record":item.raw,
                "execution":"disabled; row is not eligible for scheduling"
            });
            sqlx::query("INSERT INTO runtime_queue(id,chain_id,actor,event,status) VALUES(?,?,?,?,?)")
                .bind(format!("legacy-quarantined-{}", item.id))
                .bind(&target_chain_id)
                .bind(&prepared.owner_actor)
                .bind(storage::json_text(&wrapped)?)
                .bind("legacy-quarantined")
                .execute(&mut *tx)
                .await?;
        }

        let chain_row = sqlx::query("SELECT state FROM chains WHERE id=?")
            .bind(&target_chain_id)
            .fetch_one(&mut *tx)
            .await?;
        let chain_text: String = chain_row.try_get("state")?;
        let mut rust_agent: Value = serde_json::from_str(&chain_text)?;
        rust_agent["owners"] = json!(prepared.owners);
        rust_agent["run_state"] = json!("stopped");
        if let Some(board_id) = target_board_ids.first() {
            rust_agent["board_id"] = json!(board_id);
        }
        sqlx::query("UPDATE chains SET state=? WHERE id=?")
            .bind(storage::json_text(&rust_agent)?)
            .bind(&target_chain_id)
            .execute(&mut *tx)
            .await?;

        let manifest_id = storage::uid("legacy_upgrade_manifest");
        let manifest = json!({
            "schema":"sic-python-active-upgrade-manifest-v1",
            "source_archive_sha256":prepared.snapshot.archive_sha256,
            "original_agent":{
                "chain_id":prepared.original_chain_id,
                "state_sha256":prepared.original_chain_state_sha256,
                "run_state":prepared.original_chain_state["run_state"],
                "owners":prepared.owners,
                "disposition":"complete original record retained in raw archive; source/CB/job state is empty"
            },
            "field_policy":{
                "owners":"preserved on the target stopped Agent",
                "chain_id":"new Rust identity; original identity preserved in archive",
                "board_id_and_value":"mapped to a new read-only Board snapshot",
                "tools":"descriptions and schemas mapped individually; every operational ACL disabled",
                "source_cb_job_fields":"validated empty and remain empty",
                "runtime_queue":"each row retained in a non-runnable quarantine record",
                "archived_only":["creators","config","runtime","runtime_model_authorized","provider_inflight","events","open_tools","tool_pages","protocol_controls","inbox","delivered","sockets","instant","instant_draft","interleaves","usage","receipts","revocations","bridge_bindings","bridge_turns","host_tool_hosts","host_tool_tasks"],
                "disposition":"unmapped legacy control/session fields remain available only in the raw archive; no task or event is inferred complete"
            },
            "target_agent":{
                "chain_id":target_chain_id,
                "run_state":"stopped",
                "board_id":target_board_ids.first(),
                "model_and_runtime_work":"disabled pending explicit host review"
            },
            "boards":board_mappings,
            "tools":tool_manifest(&prepared.tools),
            "runtime_queue":{
                "quarantined_rows":prepared.queue.len(),
                "pending_or_running_not_completed":pending_or_running,
                "target_status":"legacy-quarantined",
                "execution":"disabled"
            },
            "sessions":{
                "bridge_bindings":prepared.sidecars.bridge_bindings,
                "bridge_turns":prepared.sidecars.bridge_turns,
                "host_tool_hosts":prepared.sidecars.host_tool_hosts,
                "host_tool_tasks":prepared.sidecars.host_tool_tasks,
                "disposition":"raw archive only; no live session, host, task, or lease reconstructed"
            },
            "other_tables":{
                "objects":0,
                "revocations":table(&prepared.snapshot,"revocations").map_or(0,|item|item.rows.len()),
                "receipts":table(&prepared.snapshot,"receipts").map_or(0,|item|item.rows.len()),
                "disposition":"archived without executable references"
            }
        });
        let manifest_data = storage::json_text(&manifest)?;
        let manifest_digest = storage::sha256(manifest_data.as_bytes());
        sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
            .bind(&archive_id)
            .bind("legacy_python_raw_snapshot")
            .bind(&archive_json)
            .bind(&archive_digest)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
            .bind(&manifest_id)
            .bind("legacy_python_upgrade_manifest")
            .bind(&manifest_data)
            .bind(&manifest_digest)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(UpgradeReport {
            schema: "sic-python-active-upgrade-report-v1".into(),
            target_chain_id,
            original_chain_id: prepared.original_chain_id.clone(),
            original_owner_count: prepared.owners.len(),
            imported_board_count: prepared.boards.len(),
            quarantined_tool_count: prepared.tools.len(),
            quarantined_runtime_event_count: prepared.queue.len(),
            pending_or_running_runtime_event_count: pending_or_running,
            archived_bridge_binding_count: prepared.sidecars.bridge_bindings,
            archived_bridge_turn_count: prepared.sidecars.bridge_turns,
            archived_host_tool_host_count: prepared.sidecars.host_tool_hosts,
            archived_host_tool_task_count: prepared.sidecars.host_tool_tasks,
            source_archive_object_id: archive_id,
            source_archive_sha256: prepared.snapshot.archive_sha256.clone(),
            migration_manifest_object_id: manifest_id,
            source_database_modified: false,
            target_agent_run_state: "stopped".into(),
            executable_legacy_work_resumed: false,
        })
    }
    .await;

    drop(core);
    store.close().await;
    if outcome.is_err() {
        // Remove only files created by this invocation; the source is read-only.
        for suffix in ["", "-wal", "-shm"] {
            let mut path = destination_path.as_os_str().to_owned();
            path.push(suffix);
            let _ = std::fs::remove_file(std::path::PathBuf::from(path));
        }
    }
    outcome
}
