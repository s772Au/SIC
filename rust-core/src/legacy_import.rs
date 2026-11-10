//! Strict, non-destructive importer for one narrow legacy Python state subset.
//!
//! This is not a general Python-to-Rust migration. It accepts a single stopped
//! Agent with only primary-level, plain-text pending source groups and no
//! published history or sidecar authority. The source database is opened only
//! through the read-only legacy archive APIs. Its complete raw logical snapshot
//! is retained as a non-executable object in the new Rust database.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    decision,
    error::{CoreError, Result},
    legacy::{
        LegacyRawSnapshot, RawCell, RawTable, inspect_python_database, snapshot_python_records,
    },
    storage::{self, Store},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportReport {
    pub schema: String,
    pub imported_chain_id: String,
    pub imported_source_count: usize,
    pub imported_source_ids: Vec<String>,
    pub raw_archive_object_id: String,
    pub raw_archive_sha256: String,
    pub source_database_modified: bool,
    pub executable_scope: String,
}

#[derive(Debug)]
struct LegacyObject {
    kind: String,
    value: Value,
}

fn refused(message: impl Into<String>) -> CoreError {
    CoreError::new("legacy_import_refused", message, 409)
}

fn table<'a>(snapshot: &'a LegacyRawSnapshot, name: &str) -> Result<&'a RawTable> {
    snapshot
        .tables
        .iter()
        .find(|row| row.name == name)
        .ok_or_else(|| refused(format!("legacy table {name} is missing")))
}

fn cell<'a>(row: &'a [RawCell], columns: &[String], name: &str) -> Result<&'a RawCell> {
    let index = columns
        .iter()
        .position(|column| column == name)
        .ok_or_else(|| refused(format!("legacy column {name} is missing")))?;
    row.get(index)
        .ok_or_else(|| refused("legacy row width does not match schema"))
}

fn cell_text(row: &[RawCell], columns: &[String], name: &str) -> Result<String> {
    let value = cell(row, columns, name)?;
    if value.storage_class != "TEXT" {
        return Err(refused(format!("legacy {name} must be stored as TEXT")));
    }
    let encoded = value
        .value
        .get("hex")
        .and_then(Value::as_str)
        .ok_or_else(|| refused(format!("legacy {name} raw text is unavailable")))?;
    let bytes = decode_hex(encoded)?;
    String::from_utf8(bytes).map_err(|_| refused(format!("legacy {name} is not valid UTF-8")))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(refused("legacy raw cell has odd hex length"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| -> Option<u8> {
                match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                }
            };
            let hi = digit(pair[0]).ok_or_else(|| refused("legacy raw cell has invalid hex"))?;
            let lo = digit(pair[1]).ok_or_else(|| refused("legacy raw cell has invalid hex"))?;
            Ok((hi << 4) | lo)
        })
        .collect()
}

fn parse_list(value: &Value, field: &str) -> Result<Vec<String>> {
    let values = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| refused(format!("legacy state field {field} must be an array")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    refused(format!(
                        "legacy state field {field} has an invalid identifier"
                    ))
                })
        })
        .collect()
}

fn require_empty(value: &Value, field: &str) -> Result<()> {
    if value.is_null()
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
    {
        return Ok(());
    }
    Err(refused(format!(
        "legacy state field {field} is nonempty and outside the supported subset"
    )))
}

fn validate_source(source: &Value, source_id: &str) -> Result<Vec<String>> {
    let fields = source
        .as_object()
        .ok_or_else(|| refused("source object is not a JSON object"))?;
    let allowed = BTreeSet::from(["units", "sidecar", "protected", "open_calls", "format"]);
    if fields.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err(refused(format!(
            "source {source_id} contains unsupported media, lineage, or protocol fields"
        )));
    }
    if source["format"] != "openai-chat" {
        return Err(refused(format!(
            "source {source_id} uses an unsupported format"
        )));
    }
    if source
        .get("protected")
        .and_then(Value::as_array)
        .is_none_or(|values| !values.is_empty())
    {
        return Err(refused(format!(
            "source {source_id} contains protected instructions"
        )));
    }
    if source
        .get("open_calls")
        .and_then(Value::as_array)
        .is_none_or(|values| !values.is_empty())
    {
        return Err(refused(format!(
            "source {source_id} contains an unresolved native call"
        )));
    }
    let messages = source
        .get("sidecar")
        .and_then(|value| value.get("messages"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            refused(format!(
                "source {source_id} has no replayable Chat messages"
            ))
        })?;
    let sidecar = source["sidecar"]
        .as_object()
        .ok_or_else(|| refused("source sidecar is not an object"))?;
    if sidecar.len() != 1 {
        return Err(refused(format!(
            "source {source_id} has non-message Chat controls"
        )));
    }
    let units = source
        .get("units")
        .and_then(Value::as_array)
        .ok_or_else(|| refused(format!("source {source_id} has no units array")))?;
    if units.len() != messages.len() {
        return Err(refused(format!(
            "source {source_id} is not a one-user-text-message-per-unit source"
        )));
    }
    let mut unit_ids = Vec::new();
    for (index, (message, unit)) in messages.iter().zip(units).enumerate() {
        if message.as_object().is_none_or(|fields| fields.len() != 2)
            || message["role"] != "user"
            || !message["content"].is_string()
        {
            return Err(refused(format!(
                "source {source_id} contains non-plain-text conversation history"
            )));
        }
        if unit.as_object().is_none_or(|fields| fields.len() != 4)
            || unit["anchor"]
                .as_object()
                .is_none_or(|fields| fields.len() != 2)
            || unit["unit_id"] != format!("u{}", index + 1)
            || unit["text"] != message["content"]
            || unit["kind"] != "text"
            || unit["anchor"]["message"] != index
            || unit["anchor"]["part"] != Value::Null
        {
            return Err(refused(format!(
                "source {source_id} unit/message anchors do not match"
            )));
        }
        unit_ids.push(format!("u{}", index + 1));
    }
    Ok(unit_ids)
}

fn chain_state(
    chain_id: &str,
    raw: &Value,
    objects: &BTreeMap<String, LegacyObject>,
) -> Result<(Value, Vec<String>, Vec<String>)> {
    let state = raw
        .as_object()
        .ok_or_else(|| refused("chain state is not a JSON object"))?;
    let allowed = BTreeSet::from([
        "chain_id",
        "owners",
        "creators",
        "origin_checkpoint",
        "cbs",
        "pending",
        "groups",
        "protected",
        "cut_seq",
        "current_cut",
        "instant",
        "instant_draft",
        "jobs",
        "tool_pages",
        "protocol_controls",
        "open_tools",
        "events",
        "interleaves",
        "wb_budgets",
        "run_state",
        "config",
        "hierarchy",
        "tools",
        "inbox",
        "delivered",
        "sockets",
        "usage",
        "provider_inflight",
        "waits",
        "collaborations",
        "queued_P",
        "decision",
        "runtime",
        "resource_observations",
        "editor_timing",
        "runtime_hierarchy_publications",
    ]);
    if state.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err(refused(
            "chain state contains an unknown field; preserving its executable semantics is not verified",
        ));
    }
    if raw["chain_id"] != chain_id {
        return Err(refused("chains.id does not match chain state chain_id"));
    }
    let owners = parse_list(raw, "owners")?;
    if owners.is_empty() || owners.iter().collect::<BTreeSet<_>>().len() != owners.len() {
        return Err(refused("legacy Agent owners must be nonempty and unique"));
    }
    if !parse_list(raw, "creators")?.is_empty() || !raw["origin_checkpoint"].is_null() {
        return Err(refused(
            "creator inheritance or checkpoint origin is outside the supported subset",
        ));
    }
    if raw["run_state"] != "stopped" {
        return Err(refused(
            "only an explicitly stopped legacy Agent is safe to import",
        ));
    }
    if raw["cut_seq"] != 0 || !raw["current_cut"].is_null() || raw["config"] != json!({}) {
        return Err(refused(
            "published history or nonempty runtime configuration is outside the supported subset",
        ));
    }
    for field in [
        "protected",
        "instant",
        "instant_draft",
        "jobs",
        "tool_pages",
        "protocol_controls",
        "open_tools",
        "events",
        "interleaves",
        "wb_budgets",
        "tools",
        "inbox",
        "delivered",
        "sockets",
        "usage",
        "waits",
        "collaborations",
        "queued_P",
        "runtime",
        "resource_observations",
        "editor_timing",
    ] {
        if let Some(value) = raw.get(field) {
            require_empty(value, field)?;
        }
    }
    if raw
        .get("provider_inflight")
        .is_some_and(|value| !value.is_null())
    {
        return Err(refused("legacy Provider invocation is in flight"));
    }
    if raw
        .get("runtime_hierarchy_publications")
        .is_some_and(|value| value.as_u64() != Some(0))
    {
        return Err(refused("legacy runtime publication counter is nonzero"));
    }
    if raw
        .get("decision")
        .is_some_and(|value| value != &decision::empty())
    {
        return Err(refused(
            "nonempty legacy decision state has no verified converter",
        ));
    }

    let pending = parse_list(raw, "pending")?;
    if pending.iter().collect::<BTreeSet<_>>().len() != pending.len() {
        return Err(refused("legacy pending roots are duplicated"));
    }
    let top_cbs = parse_list(raw, "cbs")?;
    if !top_cbs.is_empty() {
        return Err(refused(
            "published CB history cannot be represented by this subset",
        ));
    }
    let groups = raw
        .get("groups")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("legacy groups must be an object"))?;
    let level_hierarchy = raw
        .get("hierarchy")
        .and_then(Value::as_object)
        .ok_or_else(|| refused("legacy hierarchy is missing"))?;
    if level_hierarchy
        .keys()
        .any(|key| !["levels", "active", "contexts", "role_views"].contains(&key.as_str()))
    {
        return Err(refused("hierarchy contains unsupported fields"));
    }
    let levels = raw["hierarchy"]["levels"]
        .as_object()
        .ok_or_else(|| refused("hierarchy levels are missing"))?;
    if levels.len() != 1 || !levels.contains_key("1") {
        return Err(refused("only main primary level 1 is supported"));
    }
    let active = raw["hierarchy"]
        .get("active")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if active.iter().any(|(key, value)| {
        !["worker", "editor"].contains(&key.as_str()) || value.as_u64() != Some(1)
    }) {
        return Err(refused("non-primary legacy role selection is unsupported"));
    }
    if raw["hierarchy"]
        .get("contexts")
        .is_some_and(|value| value.as_object().is_none_or(|map| !map.is_empty()))
    {
        return Err(refused(
            "internal hierarchy contexts are outside the supported subset",
        ));
    }
    if let Some(views) = raw["hierarchy"].get("role_views") {
        let views = views
            .as_object()
            .ok_or_else(|| refused("legacy role_views must be an object"))?;
        for (role, view) in views {
            if !["worker", "editor"].contains(&role.as_str())
                || view.as_object().is_none_or(|fields| {
                    fields.len() != 2
                        || fields
                            .keys()
                            .any(|key| !["context_id", "level"].contains(&key.as_str()))
                })
                || view["context_id"] != "main"
                || view["level"].as_u64() != Some(1)
            {
                return Err(refused("legacy role view is not main level 1"));
            }
        }
    }
    let row = &raw["hierarchy"]["levels"]["1"];
    if row.as_object().is_none_or(|fields| {
        fields
            .keys()
            .any(|key| !["wbs", "cbs", "pending", "instant", "pairs"].contains(&key.as_str()))
    }) {
        return Err(refused("primary hierarchy row contains unsupported fields"));
    }
    let legacy_wbs = parse_list(row, "wbs")?;
    if !parse_list(row, "cbs")?.is_empty()
        || !row.get("pairs").is_some_and(|value| value == &json!({}))
        || !row.get("instant").is_some_and(|value| value == &json!({}))
        || parse_list(row, "pending")? != pending
    {
        return Err(refused(
            "legacy hierarchy row is not an unedited pending-only primary level",
        ));
    }
    if top_cbs != parse_list(row, "cbs")? {
        return Err(refused("top-level CB alias disagrees with hierarchy"));
    }
    if groups.len() != pending.len() || groups.keys().any(|root| !pending.contains(root)) {
        return Err(refused(
            "legacy pending list and source groups are not a one-to-one mapping",
        ));
    }
    let mut source_ids = Vec::new();
    let mut seen = BTreeSet::new();
    let mut rust_groups = BTreeMap::new();
    let mut combined_units = BTreeMap::<String, Vec<String>>::new();
    for root in &pending {
        let ids = groups[root]
            .as_array()
            .ok_or_else(|| refused("legacy source group must be an array"))?;
        if ids.is_empty() || ids[0].as_str() != Some(root.as_str()) {
            return Err(refused("legacy source group root/order is invalid"));
        }
        let mut group = Vec::new();
        for value in ids {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| refused("invalid source group id"))?;
            if !seen.insert(id.to_owned()) {
                return Err(refused("source object is referenced more than once"));
            }
            let source = objects
                .get(id)
                .ok_or_else(|| refused(format!("source group references missing object {id}")))?;
            if source.kind != "source" {
                return Err(refused(format!(
                    "source group reference {id} has the wrong object kind"
                )));
            }
            let units = validate_source(&source.value, id)?;
            for unit_id in units {
                combined_units
                    .entry(root.clone())
                    .or_default()
                    .push(if group.is_empty() {
                        unit_id
                    } else {
                        format!("{id}:{unit_id}")
                    });
            }
            group.push(id.to_owned());
            source_ids.push(id.to_owned());
        }
        rust_groups.insert(root.clone(), group);
    }
    if legacy_wbs.len() > pending.len() {
        return Err(refused("more legacy WBs than pending source roots"));
    }
    let mut imported_wbs = BTreeSet::new();
    for (index, wb_id) in legacy_wbs.iter().enumerate() {
        if !imported_wbs.insert(wb_id.clone()) {
            return Err(refused("duplicate WB ID in primary hierarchy"));
        }
        let wb = objects
            .get(wb_id)
            .ok_or_else(|| refused(format!("WB references missing immutable object {wb_id}")))?;
        if wb.kind != "wb" {
            return Err(refused(format!(
                "hierarchy WB reference {wb_id} has the wrong object kind"
            )));
        }
        let root = pending
            .get(index)
            .ok_or_else(|| refused("legacy WB has no corresponding pending source"))?;
        let expected_group = &rust_groups[root];
        let expected_units = combined_units.get(root).cloned().unwrap_or_default();
        let record = wb
            .value
            .as_object()
            .ok_or_else(|| refused("WB object is not an object"))?;
        let required_fields = BTreeSet::from([
            "wb_id",
            "chain_id",
            "context_id",
            "level",
            "sequence",
            "source_id",
            "source_ids",
            "unit_ids",
            "members",
            "reason",
            "formed_at",
        ]);
        if record.len() != required_fields.len()
            || record
                .keys()
                .any(|field| !required_fields.contains(field.as_str()))
            || wb.value["wb_id"] != *wb_id
            || wb.value["chain_id"] != chain_id
            || wb.value["context_id"] != "main"
            || wb.value["level"].as_u64() != Some(1)
            || wb.value["sequence"].as_u64() != Some((index + 1) as u64)
            || wb.value["source_id"] != *root
            || wb.value["source_ids"] != json!(expected_group)
            || wb.value["unit_ids"] != json!(expected_units)
            || wb.value["members"] != json!([])
            || !wb.value["reason"].is_string()
            || !wb.value["formed_at"].is_string()
        {
            return Err(refused(format!(
                "WB {wb_id} metadata disagrees with its pending source group"
            )));
        }
        seen.insert(wb_id.clone());
    }
    if seen.len() != objects.len() {
        return Err(refused(
            "unreferenced or unsupported immutable objects cannot be imported as executable state",
        ));
    }

    let contexts = json!({"main":{"levels":{"1":{"wbs":legacy_wbs,"cbs":[],"pending":pending,"pairs":{},"instant":{}}},"shared_contexts":[]}});
    let role_views =
        json!({"worker":{"context_id":"main","level":1},"editor":{"context_id":"main","level":1}});
    Ok((
        json!({
            "chain_id":chain_id,"owners":owners,"creators":[],"origin_checkpoint":null,"groups":rust_groups,
            "contexts":contexts,"role_views":role_views,"jobs":{},"protected":[],"current_cut":null,
            "cut_seq":0,"run_state":"stopped","config":{},"decision":decision::empty(),
            "worker_claim":null,"events":[]
        }),
        source_ids,
        parse_list(row, "wbs")?,
    ))
}

fn verify_table_shape(snapshot: &LegacyRawSnapshot) -> Result<()> {
    let required = ["objects", "chains", "boards", "revocations", "receipts"];
    for name in required {
        table(snapshot, name)?;
    }
    for (name, expected) in [
        ("objects", ["id", "kind", "data", "digest"].as_slice()),
        ("chains", ["id", "state"].as_slice()),
        ("boards", ["id", "state"].as_slice()),
        ("revocations", ["id", "record"].as_slice()),
        ("receipts", ["key", "fingerprint", "result"].as_slice()),
    ] {
        let row = table(snapshot, name)?;
        if row.columns.iter().map(String::as_str).collect::<Vec<_>>() != expected {
            return Err(refused(format!(
                "legacy table {name} has unrecognized columns"
            )));
        }
    }
    let allowed_tables = BTreeSet::from([
        "objects",
        "chains",
        "boards",
        "revocations",
        "receipts",
        "runtime_queue",
        "bridge_bindings",
        "bridge_turns",
        "host_tool_hosts",
        "host_tool_tasks",
    ]);
    if snapshot
        .tables
        .iter()
        .any(|table| !allowed_tables.contains(table.name.as_str()))
    {
        return Err(refused(
            "legacy database contains unknown tables; raw archive alone is not executable conversion",
        ));
    }
    for name in [
        "boards",
        "revocations",
        "receipts",
        "runtime_queue",
        "bridge_bindings",
        "bridge_turns",
        "host_tool_hosts",
        "host_tool_tasks",
    ] {
        if snapshot
            .tables
            .iter()
            .any(|table| table.name == name && !table.rows.is_empty())
        {
            return Err(refused(format!(
                "legacy table {name} contains sidecar or permission records"
            )));
        }
    }
    for item in &snapshot.schema_objects {
        if item["type"] == "view" {
            return Err(refused("legacy database contains an unsupported view"));
        }
        if item["type"] == "trigger" {
            let name = item["name"].as_str().unwrap_or_default();
            if ![
                "immutable_objects_update",
                "immutable_objects_delete",
                "immutable_revocations_update",
                "immutable_revocations_delete",
            ]
            .contains(&name)
            {
                return Err(refused("legacy database contains an unsupported trigger"));
            }
        }
    }
    Ok(())
}

fn extract_source_rows(
    snapshot: &LegacyRawSnapshot,
) -> Result<(
    BTreeMap<String, LegacyObject>,
    Vec<(String, String, String, String)>,
)> {
    let objects = table(snapshot, "objects")?;
    let mut sources = BTreeMap::new();
    let mut inserts = Vec::new();
    for row in &objects.rows {
        let id = cell_text(row, &objects.columns, "id")?;
        let kind = cell_text(row, &objects.columns, "kind")?;
        let data = cell_text(row, &objects.columns, "data")?;
        let digest = cell_text(row, &objects.columns, "digest")?;
        if storage::sha256(data.as_bytes()) != digest {
            return Err(refused("legacy immutable object digest mismatch"));
        }
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| refused("legacy object data is malformed JSON"))?;
        if !["source", "wb"].contains(&kind.as_str()) {
            return Err(refused(
                "only source/WB objects are supported; unknown historical object kind",
            ));
        }
        let source = LegacyObject {
            kind: kind.clone(),
            value,
        };
        if sources.insert(id.clone(), source).is_some() {
            return Err(refused("duplicate legacy object ID"));
        }
        inserts.push((id, kind, data, digest));
    }
    Ok((sources, inserts))
}

/// Import a stopped, primary-level, plain-text pending backlog into a new Rust
/// database. Existing destinations and every unsupported legacy state are
/// rejected before the destination is created. The source is never modified.
pub async fn import_stopped_text_backlog(
    source_path: &Path,
    destination_path: &Path,
) -> Result<ImportReport> {
    if !source_path.is_file() {
        return Err(CoreError::invalid("legacy database does not exist"));
    }
    if destination_path.exists() {
        return Err(CoreError::conflict("import destination already exists"));
    }
    let source_canonical = source_path
        .canonicalize()
        .map_err(|_| CoreError::invalid("cannot resolve source database path"))?;
    if let Ok(dest_canonical) = destination_path.canonicalize() {
        if dest_canonical == source_canonical {
            return Err(CoreError::conflict(
                "source database cannot be the destination",
            ));
        }
    }
    let compatibility = inspect_python_database(source_path).await?;
    if !compatibility.sqlite_integrity_ok || !compatibility.recognized_python_core_store {
        return Err(refused(
            "source is not a healthy recognized Python Core database",
        ));
    }
    let archive = snapshot_python_records(source_path).await?;
    archive.verify()?;
    verify_table_shape(&archive)?;
    let (sources, object_inserts) = extract_source_rows(&archive)?;
    let chains = table(&archive, "chains")?;
    if chains.rows.len() != 1 {
        return Err(refused(
            "supported subset requires exactly one legacy Agent",
        ));
    }
    let row = &chains.rows[0];
    let chain_id = cell_text(row, &chains.columns, "id")?;
    let raw_state = cell_text(row, &chains.columns, "state")?;
    let state_value: Value = serde_json::from_str(&raw_state)
        .map_err(|_| refused("legacy Agent state is malformed JSON"))?;
    let (rust_agent, source_ids, wb_ids) = chain_state(&chain_id, &state_value, &sources)?;
    if object_inserts.len() != sources.len() || source_ids.len() + wb_ids.len() != sources.len() {
        return Err(refused("source object accounting is incomplete"));
    }

    let archive_id = storage::uid("legacy_archive");
    let archive_value = serde_json::to_value(&archive)
        .map_err(|_| CoreError::invalid("raw archive cannot be serialized"))?;
    let archive_data = storage::json_text(&archive_value)?;
    let archive_digest = storage::sha256(archive_data.as_bytes());
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| CoreError::invalid("cannot create destination parent directory"))?;
    }
    let store = Store::create(destination_path).await?;
    let write_result: Result<()> = async {
        let mut tx = store.begin().await?;
        for (id, kind, data, digest) in &object_inserts {
            sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
                .bind(id)
                .bind(kind)
                .bind(data)
                .bind(digest)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO objects(id,kind,data,digest) VALUES(?,?,?,?)")
            .bind(&archive_id)
            .bind("legacy_python_raw_snapshot")
            .bind(&archive_data)
            .bind(&archive_digest)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO chains(id,state) VALUES(?,?)")
            .bind(&chain_id)
            .bind(storage::json_text(&rust_agent)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    store.close().await;
    write_result?;
    Ok(ImportReport {
        schema: "sic-legacy-import-report-v1".into(), imported_chain_id: chain_id,
        imported_source_count: source_ids.len(), imported_source_ids: source_ids,
        raw_archive_object_id: archive_id, raw_archive_sha256: archive.archive_sha256,
        source_database_modified: false,
        executable_scope: "one stopped Agent; main level 1; plain user-text pending sources; no published history, jobs, IPC, tools, boards, revocations, or other sidecars".into(),
    })
}
