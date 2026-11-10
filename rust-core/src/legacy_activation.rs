//! Narrow, auditable activation for a verified quarantined legacy upgrade.
//! This module can restore only the imported Board owner grant. It never
//! replays runtime events or enables archived ToolStore grants.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    board::{BoardAcl, BoardActor, BoardService},
    error::{CoreError, Result},
    ipc::{IpcState, RouteStatus},
    legacy::LegacyRawSnapshot,
    storage::{self, Store},
};

const RAW_SNAPSHOT_KIND: &str = "legacy_python_raw_snapshot";
const UPGRADE_MANIFEST_KIND: &str = "legacy_python_upgrade_manifest";
const ACTIVATION_AUDIT_KIND: &str = "legacy_permission_activation";
const START_AUDIT_KIND: &str = "legacy_agent_start";
const TARGET_QUARANTINE_STATUS: &str = "legacy-quarantined";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolMapping {
    pub legacy_name: String,
    pub disposition: String,
    pub rust_target: Option<String>,
    pub interface: String,
    pub owner_acl_verified: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationAudit {
    pub schema: String,
    pub eligible: bool,
    pub already_activated: bool,
    pub target_chain_id: String,
    pub original_chain_id: String,
    pub original_owner: String,
    pub board_id: String,
    pub board_acl_state: String,
    pub board_revision: u64,
    pub native_tool_mappings: Vec<ToolMapping>,
    pub isolated_tools: Vec<ToolMapping>,
    pub quarantined_runtime_event_count: usize,
    pub pending_or_running_event_count: usize,
    pub runtime_events_replayed: bool,
    pub raw_snapshot_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationReceipt {
    pub schema: String,
    pub target_chain_id: String,
    pub original_chain_id: String,
    pub original_owner: String,
    pub board_id: String,
    pub previous_board_revision: u64,
    pub activated_board_revision: u64,
    pub granted_board_roles: Vec<String>,
    pub native_tool_mappings: Vec<ToolMapping>,
    pub isolated_tools: Vec<ToolMapping>,
    pub quarantined_runtime_event_count: usize,
    pub pending_or_running_event_count: usize,
    pub runtime_events_replayed: bool,
    pub raw_snapshot_sha256: String,
    pub activation_audit_object_id: String,
    pub replayed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentStartReceipt {
    pub schema: String,
    pub target_chain_id: String,
    pub original_chain_id: String,
    pub original_owner: String,
    pub board_id: String,
    pub board_revision: u64,
    pub route_id: String,
    pub run_state: String,
    pub quarantined_runtime_event_count: usize,
    pub pending_or_running_event_count: usize,
    pub runtime_events_replayed: bool,
    pub raw_snapshot_sha256: String,
    pub start_audit_object_id: String,
    pub replayed: bool,
}

struct Proof {
    snapshot: LegacyRawSnapshot,
    manifest: Value,
    archive_id: String,
    manifest_id: String,
    chain_id: String,
    original_chain_id: String,
    owner: String,
    target_run_state: String,
    board_id: String,
    board_revision: u64,
    board_acl_state: String,
    raw_sha256: String,
    native_mappings: Vec<ToolMapping>,
    isolated_tools: Vec<ToolMapping>,
    queue_count: usize,
    pending_count: usize,
}

fn refused(message: impl Into<String>) -> CoreError {
    CoreError::new("legacy_activation_refused", message, 409)
}

fn exact_owner_list(value: &Value, owner: &str) -> bool {
    value
        .as_array()
        .is_some_and(|list| list.len() == 1 && list[0].as_str() == Some(owner))
}

fn raw_table<'a>(
    snapshot: &'a LegacyRawSnapshot,
    table_name: &str,
) -> Result<&'a crate::legacy::RawTable> {
    snapshot
        .tables
        .iter()
        .find(|table| table.name == table_name)
        .ok_or_else(|| refused(format!("archived source table {table_name} is missing")))
}

fn raw_text(
    table: &crate::legacy::RawTable,
    row_index: usize,
    column_name: &str,
) -> Result<String> {
    let index = table
        .columns
        .iter()
        .position(|name| name == column_name)
        .ok_or_else(|| refused(format!("archived source column {column_name} is missing")))?;
    let cell = table.rows[row_index]
        .get(index)
        .ok_or_else(|| refused("archived source row width is invalid"))?;
    if cell.storage_class != "TEXT" {
        return Err(refused(format!(
            "archived source {column_name} is not TEXT"
        )));
    }
    let hex = cell
        .value
        .get("hex")
        .and_then(Value::as_str)
        .ok_or_else(|| refused("archived source text bytes are unavailable"))?;
    if hex.len() % 2 != 0 {
        return Err(refused("archived source text has invalid hex length"));
    }
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let parse = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            let high =
                parse(pair[0]).ok_or_else(|| refused("archived source text has invalid hex"))?;
            let low =
                parse(pair[1]).ok_or_else(|| refused("archived source text has invalid hex"))?;
            Ok((high << 4) | low)
        })
        .collect::<Result<Vec<_>>>()?;
    String::from_utf8(bytes).map_err(|_| refused("archived source text is not UTF-8"))
}

fn source_chain(snapshot: &LegacyRawSnapshot, expected_chain: &str) -> Result<(String, Value)> {
    let table = raw_table(snapshot, "chains")?;
    if table.rows.len() != 1 {
        return Err(refused("source snapshot must contain exactly one Agent"));
    }
    let id = raw_text(table, 0, "id")?;
    let text = raw_text(table, 0, "state")?;
    let state: Value =
        serde_json::from_str(&text).map_err(|_| refused("archived Agent state is malformed"))?;
    if id != expected_chain || state["chain_id"].as_str() != Some(id.as_str()) {
        return Err(refused(
            "source Agent identity differs from upgrade manifest",
        ));
    }
    Ok((id, state))
}

fn map_tool(name: &str, owner_acl_verified: bool) -> ToolMapping {
    let (target, interface, reason) = match name {
        "sic_source_read" => (
            Some("sic_memory_page"),
            "worker_builtin_adapter",
            "translate source_id to kind=source and object_id; preserve offset/limit",
        ),
        "sic_memory_replay" => (
            Some("sic_memory_replay"),
            "worker_builtin_direct",
            "same name and cb_id/step arguments",
        ),
        "sic_hierarchy_expand" => (
            Some("sic_hierarchy_expand"),
            "worker_builtin_direct",
            "same name and cb_id/wb_id arguments",
        ),
        "sic_hierarchy_timeline" => (
            Some("sic_hierarchy_timeline"),
            "worker_builtin_direct",
            "same name and context_id/level/kind/offset/limit arguments",
        ),
        "sic_decision_update" => (
            Some("decision.update"),
            "host_operation_only",
            "same expected_revision/events payload; not exposed as a Worker tool",
        ),
        "sic_agent_routes" => (
            Some("chain.list"),
            "host_operation_only",
            "host Core operation is actor-scoped; not exposed as a Worker tool",
        ),
        "sic_memory_page" => (
            None,
            "isolated",
            "legacy schema includes level/context_id/role controls rejected by the Rust sic_memory_page builtin",
        ),
        "sic_source_search" => (
            None,
            "isolated",
            "no verified Rust Worker source-search builtin with equivalent paging and excerpt semantics",
        ),
        "sic_operation_directory" => (
            None,
            "isolated",
            "no verified Rust builtin returns the legacy chronological CB operation directory",
        ),
        "sic_hierarchy_legacy" => (
            None,
            "isolated",
            "Rust target has no equivalent legacy chronology view for this imported empty history",
        ),
        "sic_memory_timeline" => (
            None,
            "isolated",
            "Rust hierarchy timeline is scoped by context and level; equivalence to full legacy memory timeline is unverified",
        ),
        "sic_memory_locate" => (
            None,
            "isolated",
            "no verified Rust builtin preserves the source_id/cb_id relationship lookup contract",
        ),
        "sic_socket_dispatch" => (
            None,
            "isolated",
            "legacy multi-route collaboration dispatch is not implemented by the Rust Core",
        ),
        "sic_socket_signal" => (
            None,
            "isolated",
            "legacy run/wait/wake/stop schema is not proven equivalent to the Rust pause/resume/wake/stop action contract",
        ),
        "sic_collaboration_status" => (
            None,
            "isolated",
            "Rust socket inbox does not provide equivalent collaboration request/result status",
        ),
        _ => (None, "isolated", "legacy tool has no reviewed Rust mapping"),
    };
    let disposition = if !owner_acl_verified {
        "isolated_acl_differs_from_unique_owner"
    } else if target.is_some() {
        "mapping_recorded_not_enabled"
    } else {
        "isolated_no_verified_equivalent"
    };
    ToolMapping {
        legacy_name: name.to_owned(),
        disposition: disposition.into(),
        rust_target: target.map(str::to_owned),
        interface: interface.into(),
        owner_acl_verified,
        reason: reason.into(),
    }
}

fn decode_mappings(state: &Value, owner: &str) -> Result<(Vec<ToolMapping>, Vec<ToolMapping>)> {
    let tools = state
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("source Agent tools are missing"))?;
    let mut seen = BTreeSet::new();
    let mut native = Vec::new();
    let mut isolated = Vec::new();
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| refused("source tool has no name"))?;
        if !seen.insert(name.to_owned()) {
            return Err(refused("source tool names are not unique"));
        }
        let owner_acl_verified = ["discover_acl", "expand_acl", "execute_acl"]
            .iter()
            .all(|field| exact_owner_list(&tool[*field], owner));
        let mapping = map_tool(name, owner_acl_verified);
        if mapping.rust_target.is_some() && owner_acl_verified {
            native.push(mapping);
        } else {
            isolated.push(mapping);
        }
    }
    Ok((native, isolated))
}

fn decode_queue(snapshot: &LegacyRawSnapshot) -> Result<(BTreeMap<String, String>, usize)> {
    let table = raw_table(snapshot, "runtime_queue")?;
    let required = ["id", "chain_id", "actor", "event", "status"];
    if table.columns.iter().map(String::as_str).collect::<Vec<_>>() != required {
        return Err(refused("source runtime_queue shape is unsupported"));
    }
    let mut expected = BTreeMap::new();
    let mut pending = 0usize;
    for (index, row) in table.rows.iter().enumerate() {
        let get = |name: &str| -> Result<&crate::legacy::RawCell> {
            let col = table
                .columns
                .iter()
                .position(|column| column == name)
                .ok_or_else(|| refused(format!("runtime_queue column {name} is missing")))?;
            row.get(col)
                .ok_or_else(|| refused("runtime_queue row width is invalid"))
        };
        let text_cell = |name: &str| -> Result<String> {
            let cell = get(name)?;
            if cell.storage_class != "TEXT" {
                return Err(refused(format!("runtime_queue {name} must be TEXT")));
            }
            raw_text(table, index, name)
        };
        let id = text_cell("id")?;
        if id.is_empty() {
            return Err(refused("source runtime_queue has an empty ID"));
        }
        let status = text_cell("status")?;
        if matches!(status.as_str(), "pending" | "running") {
            pending += 1;
        }
        let digest = table.row_sha256[index].clone();
        if expected.insert(digest, status).is_some() {
            return Err(refused("runtime_queue row digest collision"));
        }
    }
    Ok((expected, pending))
}

fn array_empty(value: &Value) -> bool {
    value.as_array().is_some_and(Vec::is_empty)
}

fn object_empty(value: &Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
}

fn target_context_work_empty(value: &Value) -> bool {
    let Some(contexts) = value.get("contexts").and_then(Value::as_object) else {
        return false;
    };
    contexts.values().all(|context| {
        if !array_empty(&context["shared_contexts"]) {
            return false;
        }
        let Some(levels) = context.get("levels").and_then(Value::as_object) else {
            return false;
        };
        levels.values().all(|level| {
            array_empty(&level["cbs"])
                && array_empty(&level["pending"])
                && array_empty(&level["wbs"])
                && object_empty(&level["pairs"])
                && object_empty(&level["instant"])
        })
    })
}

async fn object_rows(
    tx: &mut Transaction<'_, Sqlite>,
    kind: &str,
) -> Result<Vec<(String, String, String)>> {
    let rows = sqlx::query("SELECT id,data,digest FROM objects WHERE kind=? ORDER BY id")
        .bind(kind)
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("id")?,
                row.try_get("data")?,
                row.try_get("digest")?,
            ))
        })
        .collect()
}

async fn verify_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    target_chain_id: &str,
) -> Result<Proof> {
    let manifests = object_rows(tx, UPGRADE_MANIFEST_KIND).await?;
    let mut found_manifest = None;
    for (id, data, digest) in manifests {
        if storage::sha256(data.as_bytes()) != digest {
            return Err(refused("upgrade manifest integrity check failed"));
        }
        let manifest: Value = serde_json::from_str(&data)
            .map_err(|_| refused("upgrade manifest JSON is malformed"))?;
        if manifest
            .pointer("/target_agent/chain_id")
            .and_then(Value::as_str)
            == Some(target_chain_id)
        {
            if found_manifest.is_some() {
                return Err(refused("multiple upgrade manifests target this Agent"));
            }
            found_manifest = Some((id, manifest));
        }
    }
    let (manifest_id, manifest) =
        found_manifest.ok_or_else(|| refused("no verified upgrade manifest for target Agent"))?;
    let expected_archive_sha = manifest["source_archive_sha256"]
        .as_str()
        .ok_or_else(|| refused("upgrade manifest has no source archive digest"))?
        .to_owned();
    let archives = object_rows(tx, RAW_SNAPSHOT_KIND).await?;
    let mut found_archive = None;
    for (id, data, digest) in archives {
        if storage::sha256(data.as_bytes()) != digest {
            return Err(refused("raw archive object digest mismatch"));
        }
        let snapshot: LegacyRawSnapshot =
            serde_json::from_str(&data).map_err(|_| refused("raw archive JSON is malformed"))?;
        snapshot.verify()?;
        if snapshot.archive_sha256 == expected_archive_sha {
            if found_archive.is_some() {
                return Err(refused("multiple raw archives match this upgrade manifest"));
            }
            found_archive = Some((id, snapshot));
        }
    }
    let (archive_id, snapshot) =
        found_archive.ok_or_else(|| refused("upgrade raw archive is unavailable"))?;
    let (original_chain_id, source_state) = source_chain(
        &snapshot,
        manifest["original_agent"]["chain_id"]
            .as_str()
            .ok_or_else(|| refused("manifest has no original chain ID"))?,
    )?;
    if source_state["run_state"].as_str() != Some("running") {
        return Err(refused("archive is not for an active Python Agent"));
    }
    let owners = source_state
        .get("owners")
        .and_then(Value::as_array)
        .ok_or_else(|| refused("source Agent owner list is missing"))?;
    if owners.len() != 1 {
        return Err(refused("activation requires exactly one archived owner"));
    }
    let owner = owners[0]
        .as_str()
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| refused("source Agent owner is invalid"))?
        .to_owned();
    if manifest.pointer("/original_agent/owners") != Some(&json!([owner])) {
        return Err(refused("upgrade manifest owner differs from raw snapshot"));
    }
    if !array_empty(&source_state["cbs"])
        || !array_empty(&source_state["pending"])
        || !object_empty(&source_state["groups"])
        || !object_empty(&source_state["jobs"])
        || source_state
            .get("current_cut")
            .is_some_and(|cut| !cut.is_null())
    {
        return Err(refused(
            "source Agent has nonempty history or outstanding work",
        ));
    }

    let object_kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM objects ORDER BY kind")
        .fetch_all(&mut **tx)
        .await?;
    if object_kinds.iter().any(|kind| {
        !matches!(
            kind.as_str(),
            RAW_SNAPSHOT_KIND | UPGRADE_MANIFEST_KIND | ACTIVATION_AUDIT_KIND | START_AUDIT_KIND
        )
    }) {
        return Err(refused(
            "target Agent has executable source/history objects",
        ));
    }
    let chain_row = sqlx::query("SELECT state FROM chains WHERE id=?")
        .bind(target_chain_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| refused("target Rust Agent is missing"))?;
    let chain_text: String = chain_row.try_get("state")?;
    let target_state: Value = serde_json::from_str(&chain_text)
        .map_err(|_| refused("target Rust Agent state is malformed"))?;
    let target_run_state = target_state["run_state"]
        .as_str()
        .ok_or_else(|| refused("target Agent run state is missing"))?
        .to_owned();
    if target_run_state != "stopped"
        || target_state.get("owners") != Some(&json!([owner]))
        || !target_context_work_empty(&target_state)
        || !object_empty(&target_state["jobs"])
        || !array_empty(&target_state["events"])
        || !array_empty(&target_state["protected"])
        || target_state["cut_seq"].as_u64() != Some(0)
        || target_state
            .get("current_cut")
            .is_some_and(|cut| !cut.is_null())
        || target_state
            .get("worker_claim")
            .is_some_and(|claim| !claim.is_null())
    {
        return Err(refused("target Agent is not stopped and empty"));
    }

    let board_mappings = manifest["boards"]
        .as_array()
        .filter(|boards| boards.len() == 1)
        .ok_or_else(|| refused("manifest must contain exactly one mapped Board"))?;
    let source_board_id = board_mappings[0]["source_board_id"]
        .as_str()
        .ok_or_else(|| refused("Board mapping has no source ID"))?;
    let board_id = board_mappings[0]["target_board_id"]
        .as_str()
        .ok_or_else(|| refused("Board mapping has no target ID"))?
        .to_owned();
    if target_state["board_id"].as_str() != Some(&board_id) {
        return Err(refused("target Agent Board binding differs from manifest"));
    }
    let source_boards = raw_table(&snapshot, "boards")?;
    if source_boards.rows.len() != 1 {
        return Err(refused("source archive must have exactly one Board"));
    }
    let archived_board_id = raw_text(source_boards, 0, "id")?;
    let source_board_text = raw_text(source_boards, 0, "state")?;
    let source_board: Value = serde_json::from_str(&source_board_text)
        .map_err(|_| refused("source Board is malformed"))?;
    if archived_board_id != source_board_id
        || source_board["owner"].as_str() != Some(owner.as_str())
        || !exact_owner_list(&source_board["read_acl"], &owner)
        || !exact_owner_list(&source_board["write_acl"], &owner)
    {
        return Err(refused(
            "source Board read/write ACLs must each contain only the unique Agent owner",
        ));
    }
    if source_state.get("board_id").and_then(Value::as_str) != Some(source_board_id) {
        return Err(refused(
            "source Agent Board reference differs from archived Board",
        ));
    }

    let board_row =
        sqlx::query("SELECT owner_actor,revision,acl_json FROM sic_boards WHERE board_id=?")
            .bind(&board_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| refused("target Board is missing"))?;
    let board_owner: String = board_row.try_get("owner_actor")?;
    let board_revision: i64 = board_row.try_get("revision")?;
    let acl_json: String = board_row.try_get("acl_json")?;
    let current_acl: Value =
        serde_json::from_str(&acl_json).map_err(|_| refused("target Board ACL is malformed"))?;
    let target_chain_reader = exact_owner_list(&current_acl["readers"], target_chain_id);
    let restricted_acl = target_chain_reader
        && array_empty(&current_acl["writers"])
        && array_empty(&current_acl["claimers"])
        && array_empty(&current_acl["managers"]);
    let active_acl = target_chain_reader
        && exact_owner_list(&current_acl["writers"], target_chain_id)
        && exact_owner_list(&current_acl["claimers"], target_chain_id)
        && exact_owner_list(&current_acl["managers"], target_chain_id);
    if board_owner != target_chain_id || !(restricted_acl || active_acl) {
        return Err(refused(
            "target Board ACL is neither the migration quarantine nor the audited owner grant",
        ));
    }
    let acl_state = if restricted_acl {
        "restricted"
    } else {
        "activated"
    };
    if (restricted_acl && board_revision != 3) || (active_acl && board_revision != 4) {
        return Err(refused(
            "target Board revision changed outside the audited activation",
        ));
    }
    let principal = BoardActor::from_authenticated_host(target_chain_id)?;
    let current_i = BoardService::read_latest_i_in_transaction(tx, &principal, &board_id).await?;
    let entry = current_i["entries"]
        .as_array()
        .filter(|entries| entries.len() == 1)
        .and_then(|entries| entries.first())
        .ok_or_else(|| refused("target Board snapshot entries differ from the verified upgrade"))?;
    let content = &entry["content"];
    if content["legacy_board_id"].as_str() != Some(source_board_id)
        || content["legacy_revision"] != source_board["revision"]
        || content["legacy_owner"] != source_board["owner"]
        || content["legacy_acl"]["read_acl"] != source_board["read_acl"]
        || content["legacy_acl"]["write_acl"] != source_board["write_acl"]
        || content["value"] != source_board["value"]
        || !current_i["tasks"].as_array().is_some_and(Vec::is_empty)
    {
        return Err(refused(
            "target Board content differs from the archived read-only snapshot",
        ));
    }

    let (_, source_pending_count) = decode_queue(&snapshot)?;
    let expected_queue = decode_queue(&snapshot)?.0;
    let target_queue_rows = sqlx::query("SELECT status,event FROM runtime_queue WHERE chain_id=?")
        .bind(target_chain_id)
        .fetch_all(&mut **tx)
        .await?;
    if target_queue_rows.len() != expected_queue.len() {
        return Err(refused(
            "quarantined runtime queue row count differs from archive",
        ));
    }
    let mut seen_queue = BTreeSet::new();
    for row in target_queue_rows {
        let status: String = row.try_get("status")?;
        let event_text: String = row.try_get("event")?;
        let event: Value = serde_json::from_str(&event_text)
            .map_err(|_| refused("quarantined runtime event is malformed"))?;
        let row_digest = event
            .pointer("/raw_source_record/row_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("quarantined runtime event has no source row digest"))?;
        let original_status = event["original_status"]
            .as_str()
            .ok_or_else(|| refused("quarantined runtime event has no original status"))?;
        if status != TARGET_QUARANTINE_STATUS
            || expected_queue.get(row_digest).map(String::as_str) != Some(original_status)
            || !seen_queue.insert(row_digest.to_owned())
        {
            return Err(refused("runtime queue is altered or no longer quarantined"));
        }
    }
    if seen_queue.len() != expected_queue.len() {
        return Err(refused("one or more source runtime events are missing"));
    }

    let tables_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bridge_bindings")
        .fetch_one(&mut **tx)
        .await?;
    let bridge_turns_empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bridge_turns")
        .fetch_one(&mut **tx)
        .await?;
    let host_rows_empty: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM host_tool_hosts) + (SELECT COUNT(*) FROM host_tool_tasks)",
    )
    .fetch_one(&mut **tx)
    .await?;
    if tables_empty != 0 || bridge_turns_empty != 0 || host_rows_empty != 0 {
        return Err(refused(
            "target has active session, host, or tool-task state",
        ));
    }

    let tool_row =
        sqlx::query("SELECT actor,state,digest FROM sic_dynamic_toolsets WHERE chain_id=?")
            .bind(target_chain_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| refused("quarantined ToolStore is missing"))?;
    let tool_actor: String = tool_row.try_get("actor")?;
    let tool_state_text: String = tool_row.try_get("state")?;
    let tool_state_digest: String = tool_row.try_get("digest")?;
    if tool_actor != owner || storage::sha256(tool_state_text.as_bytes()) != tool_state_digest {
        return Err(refused(
            "quarantined ToolStore identity or digest is invalid",
        ));
    }
    let tool_state: Value = serde_json::from_str(&tool_state_text)
        .map_err(|_| refused("quarantined ToolStore state is malformed"))?;
    if tool_state
        .pointer("/acl/discover")
        .and_then(Value::as_array)
        .is_none_or(|acl| !acl.is_empty())
        || tool_state
            .pointer("/acl/expand")
            .and_then(Value::as_array)
            .is_none_or(|acl| !acl.is_empty())
        || tool_state
            .pointer("/acl/execute")
            .and_then(Value::as_array)
            .is_none_or(|acl| !acl.is_empty())
    {
        return Err(refused("quarantined ToolStore has live grants"));
    }
    let tools = tool_state["tools"]
        .as_object()
        .ok_or_else(|| refused("quarantined ToolStore has no tool catalog"))?;
    if tools.len() != source_state["tools"].as_array().map_or(0, Vec::len) {
        return Err(refused("quarantined ToolStore catalog differs from source"));
    }
    for (name, tool) in tools {
        if tool["expanded"] == true
            || tool["host_fixed"] == true
            || tool["in_flight"]
                .as_array()
                .is_none_or(|values| !values.is_empty())
        {
            return Err(refused(format!("quarantined tool {name} is active")));
        }
    }
    let native_and_isolated = decode_mappings(&source_state, &owner)?;
    Ok(Proof {
        snapshot,
        manifest,
        archive_id,
        manifest_id,
        chain_id: target_chain_id.into(),
        original_chain_id,
        owner,
        target_run_state,
        board_id,
        board_revision: board_revision as u64,
        board_acl_state: acl_state.into(),
        raw_sha256: expected_archive_sha,
        native_mappings: native_and_isolated.0,
        isolated_tools: native_and_isolated.1,
        queue_count: expected_queue.len(),
        pending_count: source_pending_count,
    })
}

fn audit_from(proof: &Proof) -> ActivationAudit {
    ActivationAudit {
        schema: "sic-legacy-activation-audit-v1".into(),
        eligible: proof.board_acl_state == "restricted",
        already_activated: proof.board_acl_state == "activated",
        target_chain_id: proof.chain_id.clone(),
        original_chain_id: proof.original_chain_id.clone(),
        original_owner: proof.owner.clone(),
        board_id: proof.board_id.clone(),
        board_acl_state: proof.board_acl_state.clone(),
        board_revision: proof.board_revision,
        native_tool_mappings: proof.native_mappings.clone(),
        isolated_tools: proof.isolated_tools.clone(),
        quarantined_runtime_event_count: proof.queue_count,
        pending_or_running_event_count: proof.pending_count,
        runtime_events_replayed: false,
        raw_snapshot_sha256: proof.raw_sha256.clone(),
    }
}

/// Verify the recorded migration and current target state without changing it.
pub async fn audit_quarantined_activation(
    store: &Store,
    target_chain_id: &str,
    authenticated_actor: &str,
) -> Result<ActivationAudit> {
    let mut tx = store.begin().await?;
    let proof = verify_in_transaction(&mut tx, target_chain_id).await?;
    if proof.owner != authenticated_actor {
        return Err(CoreError::forbidden("legacy Agent owner required"));
    }
    let audit = audit_from(&proof);
    tx.commit().await?;
    Ok(audit)
}

/// Restore only the owner-equivalent Board ACL. Runtime events stay
/// quarantined, ToolStore ACLs stay empty, and the stopped Agent stays stopped.
/// The CAS revision and idempotency receipt make concurrent/retried activation
/// auditable and prevent an implicit second grant transition.
pub async fn activate_verified_owner_board(
    store: &Store,
    target_chain_id: &str,
    authenticated_actor: &str,
    expected_board_revision: u64,
    idempotency_key: &str,
) -> Result<ActivationReceipt> {
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 128 {
        return Err(CoreError::invalid("activation idempotency key is invalid"));
    }
    let receipt_key = format!("legacy-owner-board-activation:{target_chain_id}:{idempotency_key}");
    let fingerprint = storage::digest(&json!({
        "target_chain_id":target_chain_id,
        "expected_board_revision":expected_board_revision,
        "idempotency_key":idempotency_key,
        "activation":"owner-equivalent-board-acl-v1"
    }))?;
    let mut tx = store.begin().await?;
    if let Some(row) = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?")
        .bind(&receipt_key)
        .fetch_optional(&mut *tx)
        .await?
    {
        let stored_fingerprint: String = row.try_get("fingerprint")?;
        if stored_fingerprint != fingerprint {
            return Err(CoreError::conflict(
                "activation key was reused with a different request",
            ));
        }
        let stored_result: String = row.try_get("result")?;
        let mut receipt: ActivationReceipt =
            serde_json::from_str(&stored_result).map_err(|_| {
                CoreError::new(
                    "legacy_activation_receipt_invalid",
                    "activation receipt is malformed",
                    500,
                )
            })?;
        if receipt.original_owner != authenticated_actor {
            return Err(CoreError::forbidden("legacy Agent owner required"));
        }
        receipt.replayed = true;
        tx.commit().await?;
        return Ok(receipt);
    }

    let proof = verify_in_transaction(&mut tx, target_chain_id).await?;
    if proof.owner != authenticated_actor {
        return Err(CoreError::forbidden("legacy Agent owner required"));
    }
    if proof.board_acl_state != "restricted" {
        return Err(refused(
            "target Board is not in the restricted pre-activation state",
        ));
    }
    if proof.board_revision != expected_board_revision {
        return Err(CoreError::conflict(
            "target Board revision changed before activation",
        ));
    }
    let actor = BoardActor::from_authenticated_host(target_chain_id)?;
    let acl = BoardAcl::new(
        [target_chain_id.to_owned()],
        [target_chain_id.to_owned()],
        [target_chain_id.to_owned()],
        [target_chain_id.to_owned()],
    )?;
    BoardService::update_acl_from_host_in_transaction(
        &mut tx,
        &actor,
        &proof.board_id,
        expected_board_revision,
        "legacy-owner-equivalent-activation",
        acl,
    )
    .await?;
    let audit_object_id = storage::uid("legacy_activation");
    let mut receipt = ActivationReceipt {
        schema: "sic-legacy-activation-receipt-v1".into(),
        target_chain_id: proof.chain_id.clone(),
        original_chain_id: proof.original_chain_id.clone(),
        original_owner: proof.owner.clone(),
        board_id: proof.board_id.clone(),
        previous_board_revision: expected_board_revision,
        activated_board_revision: expected_board_revision + 1,
        granted_board_roles: vec![
            "read".into(),
            "write".into(),
            "claim".into(),
            "manage".into(),
        ],
        native_tool_mappings: proof.native_mappings.clone(),
        isolated_tools: proof.isolated_tools.clone(),
        quarantined_runtime_event_count: proof.queue_count,
        pending_or_running_event_count: proof.pending_count,
        runtime_events_replayed: false,
        raw_snapshot_sha256: proof.raw_sha256.clone(),
        activation_audit_object_id: audit_object_id.clone(),
        replayed: false,
    };
    let audit_data = storage::json_text(&json!({
        "receipt":receipt,
        "upgrade_manifest_object_id":proof.manifest_id,
        "raw_snapshot_object_id":proof.archive_id,
        "source_snapshot_archive_sha256":proof.snapshot.archive_sha256,
        "target_manifest":proof.manifest,
        "queue_action":"none; all events remain legacy-quarantined"
    }))?;
    let audit_digest = storage::sha256(audit_data.as_bytes());
    sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
        .bind(&audit_object_id)
        .bind(ACTIVATION_AUDIT_KIND)
        .bind(&audit_data)
        .bind(&audit_digest)
        .execute(&mut *tx)
        .await?;
    receipt.replayed = false;
    let result = storage::json_text(
        &serde_json::to_value(&receipt)
            .map_err(|_| CoreError::invalid("activation receipt cannot be serialized"))?,
    )?;
    sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)")
        .bind(receipt_key)
        .bind(fingerprint)
        .bind(result)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(receipt)
}

/// Start an activated legacy Agent only after revalidating its archived owner,
/// activated Board, empty target state, quarantined queue, and existing IPC
/// route. The route and run-state transition are checked and committed in the
/// same transaction; no queue, ToolStore grant, provider, or IPC route state is
/// changed.
pub async fn start_activated_agent(
    store: &Store,
    target_chain_id: &str,
    authenticated_actor: &str,
    idempotency_key: &str,
) -> Result<AgentStartReceipt> {
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 128 {
        return Err(CoreError::invalid("start idempotency key is invalid"));
    }
    let receipt_key = format!("legacy-agent-start:{target_chain_id}:{idempotency_key}");
    let fingerprint = storage::digest(&json!({
        "target_chain_id":target_chain_id,
        "authenticated_actor":authenticated_actor,
        "idempotency_key":idempotency_key,
        "operation":"start-activated-legacy-agent-v1"
    }))?;
    let mut tx = store.begin().await?;
    if let Some(row) = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?")
        .bind(&receipt_key)
        .fetch_optional(&mut *tx)
        .await?
    {
        let stored_fingerprint: String = row.try_get("fingerprint")?;
        let stored_result: String = row.try_get("result")?;
        let mut receipt: AgentStartReceipt =
            serde_json::from_str(&stored_result).map_err(|_| {
                CoreError::new(
                    "legacy_start_receipt_invalid",
                    "start receipt is malformed",
                    500,
                )
            })?;
        if receipt.target_chain_id != target_chain_id
            || receipt.original_owner != authenticated_actor
            || receipt.route_id != target_chain_id
            || receipt.run_state != "running"
            || receipt.schema != "sic-legacy-agent-start-receipt-v1"
        {
            return Err(CoreError::forbidden("legacy Agent owner required"));
        }
        let chain_row = sqlx::query("SELECT state FROM chains WHERE id=?")
            .bind(target_chain_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| CoreError::forbidden("authenticated Agent route is unavailable"))?;
        let chain_text: String = chain_row.try_get("state")?;
        let chain_state: Value = serde_json::from_str(&chain_text)
            .map_err(|_| refused("current Agent state is malformed"))?;
        if chain_state["chain_id"].as_str() != Some(target_chain_id)
            || !exact_owner_list(&chain_state["owners"], authenticated_actor)
        {
            return Err(CoreError::forbidden(
                "current Agent owner authorization required",
            ));
        }
        if stored_fingerprint != fingerprint {
            return Err(CoreError::conflict(
                "start key was reused with a different request",
            ));
        }
        receipt.replayed = true;
        tx.commit().await?;
        return Ok(receipt);
    }

    let proof = verify_in_transaction(&mut tx, target_chain_id).await?;
    if proof.owner != authenticated_actor {
        return Err(CoreError::forbidden("legacy Agent owner required"));
    }
    if proof.board_acl_state != "activated" {
        return Err(refused(
            "owner Board permissions must be activated before start",
        ));
    }

    let route_row = sqlx::query("SELECT state,digest FROM sic_rust_ipc_states WHERE namespace=?")
        .bind("sic-agent-workspace")
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| refused("IPC route state is missing"))?;
    let ipc_text: String = route_row.try_get("state")?;
    let ipc_digest: String = route_row.try_get("digest")?;
    if storage::sha256(ipc_text.as_bytes()) != ipc_digest {
        return Err(refused("IPC route state digest is invalid"));
    }
    let ipc_state: IpcState =
        serde_json::from_str(&ipc_text).map_err(|_| refused("IPC route state is malformed"))?;
    let route = ipc_state
        .route(target_chain_id)
        .ok_or_else(|| refused("verified IPC route is missing"))?;
    if route.id() != target_chain_id
        || route.owner_actor_id() != authenticated_actor
        || route.owners().len() != 1
        || route.owners()[0] != authenticated_actor
        || route.status() != &RouteStatus::Running
    {
        return Err(refused("IPC route identity, owner, or status is invalid"));
    }

    if proof.target_run_state != "stopped" {
        return Err(refused("Agent is not stopped for its first start"));
    }
    let row = sqlx::query("SELECT state FROM chains WHERE id=?")
        .bind(target_chain_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| refused("target Agent is missing"))?;
    let state_text: String = row.try_get("state")?;
    let mut state: Value = serde_json::from_str(&state_text)
        .map_err(|_| refused("target Agent state is malformed"))?;
    state["run_state"] = json!("running");
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(storage::json_text(&state)?)
        .bind(target_chain_id)
        .execute(&mut *tx)
        .await?;

    let audit_object_id = storage::uid("legacy_agent_start");
    let mut receipt = AgentStartReceipt {
        schema: "sic-legacy-agent-start-receipt-v1".into(),
        target_chain_id: proof.chain_id.clone(),
        original_chain_id: proof.original_chain_id.clone(),
        original_owner: proof.owner.clone(),
        board_id: proof.board_id.clone(),
        board_revision: proof.board_revision,
        route_id: target_chain_id.to_owned(),
        run_state: "running".into(),
        quarantined_runtime_event_count: proof.queue_count,
        pending_or_running_event_count: proof.pending_count,
        runtime_events_replayed: false,
        raw_snapshot_sha256: proof.raw_sha256.clone(),
        start_audit_object_id: audit_object_id.clone(),
        replayed: false,
    };
    let audit_data = storage::json_text(&json!({
        "receipt":receipt,
        "activation_audit_kind":ACTIVATION_AUDIT_KIND,
        "route_validation":"existing running owner route; unchanged",
        "queue_action":"none; all events remain legacy-quarantined",
        "toolstore_action":"none; all legacy grants remain disabled",
        "provider_action":"none"
    }))?;
    let audit_digest = storage::sha256(audit_data.as_bytes());
    sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
        .bind(&audit_object_id)
        .bind(START_AUDIT_KIND)
        .bind(&audit_data)
        .bind(&audit_digest)
        .execute(&mut *tx)
        .await?;
    receipt.replayed = false;
    let result = storage::json_text(
        &serde_json::to_value(&receipt)
            .map_err(|_| CoreError::invalid("start receipt cannot be serialized"))?,
    )?;
    sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)")
        .bind(receipt_key)
        .bind(fingerprint)
        .bind(result)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(receipt)
}
