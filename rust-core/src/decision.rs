//! Pure four-view decision workspace transitions.
//!
//! Events and Editor replacements are proposals supplied by a trusted caller;
//! this module performs shape/CAS checks only and never infers that work ran.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const VIEWS: [&str; 4] = ["Working on", "Failed Before", "Not Tried Yet", "Overlooked"];

fn err(message: &str) -> String {
    message.to_owned()
}

/// Construct the empty, explicitly incomplete four-view workspace.
pub fn empty() -> Value {
    let mut views = Map::new();
    for name in VIEWS {
        views.insert(name.to_owned(), Value::Array(Vec::new()));
    }
    json!({
        "revision": 0,
        "views": Value::Object(views),
        "discovery_prompt": "What have I missed?",
        "event_ids": []
    })
}

fn digest(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|_| err("decision value cannot be encoded"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| err(&format!("{name} must be an object")))
}

fn only_keys(value: &Map<String, Value>, allowed: &[&str], name: &str) -> Result<(), String> {
    if value.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(err(&format!("unknown {name} field")));
    }
    Ok(())
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| err(&format!("{name} must be a string")))
}

fn validate_row(row: &Value, name: &str, editor: bool) -> Result<(), String> {
    let fields = object(row, name)?;
    let allowed = if editor {
        &["id", "text", "evidence", "status"][..]
    } else {
        &[
            "id",
            "text",
            "evidence",
            "status",
            "completion",
            "completion_evidence",
        ][..]
    };
    only_keys(fields, allowed, name)?;
    let id = fields
        .get("id")
        .ok_or_else(|| err(&format!("{name} requires stable id")))?;
    if string(id, &format!("{name} id"))?.trim().is_empty() {
        return Err(err("decision id must be nonempty"));
    }
    string(
        fields
            .get("text")
            .ok_or_else(|| err(&format!("{name} requires text")))?,
        &format!("{name} text"),
    )?;
    if !fields.get("evidence").is_some_and(Value::is_array) {
        return Err(err(&format!("{name} evidence must be an array")));
    }
    let status = string(
        fields
            .get("status")
            .ok_or_else(|| err(&format!("{name} requires status")))?,
        &format!("{name} status"),
    )?;
    if !["active", "recorded", "completed"].contains(&status) {
        return Err(err("unknown decision item status"));
    }
    if editor && status == "completed" {
        return Err(err("Editor cannot assert completed work"));
    }
    if !editor {
        if let Some(completion) = fields.get("completion") {
            string(completion, "completion text")?;
        }
        if let Some(evidence) = fields.get("completion_evidence") {
            if !evidence.is_array() {
                return Err(err("completion evidence must be an array"));
            }
        }
        if status == "completed"
            && (!fields.contains_key("completion") || !fields.contains_key("completion_evidence"))
        {
            return Err(err("completed work requires completion text and evidence"));
        }
    }
    Ok(())
}

fn validate_current(current: &Value) -> Result<u64, String> {
    let fields = object(current, "decision workspace")?;
    only_keys(
        fields,
        &["revision", "views", "discovery_prompt", "event_ids"],
        "decision workspace",
    )?;
    let revision = fields
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| err("decision revision must be a nonnegative integer"))?;
    string(
        fields
            .get("discovery_prompt")
            .ok_or_else(|| err("decision workspace requires discovery prompt"))?,
        "discovery prompt",
    )?;
    let events = fields
        .get("event_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| err("decision event_ids must be an array"))?;
    let mut event_ids = BTreeSet::new();
    for id in events {
        let id = string(id, "event id")?;
        if id.trim().is_empty() || !event_ids.insert(id) {
            return Err(err("decision event ids must be nonempty and unique"));
        }
    }
    let views = object(
        fields
            .get("views")
            .ok_or_else(|| err("decision workspace requires views"))?,
        "decision views",
    )?;
    only_keys(views, &VIEWS, "decision view")?;
    if views.len() != VIEWS.len() || VIEWS.iter().any(|name| !views.contains_key(*name)) {
        return Err(err("decision workspace requires all four views"));
    }
    let mut item_ids = BTreeSet::new();
    for (name, rows) in views {
        let rows = rows
            .as_array()
            .ok_or_else(|| err("decision view entries must be arrays"))?;
        for row in rows {
            validate_row(row, name, false)?;
            let id = row["id"].as_str().unwrap();
            if !item_ids.insert(id.to_owned()) {
                return Err(err("decision item ids must be unique"));
            }
        }
    }
    Ok(revision)
}

fn view_for(kind: &str) -> Option<&'static str> {
    match kind {
        "work" => Some("Working on"),
        "failure" => Some("Failed Before"),
        "candidate" => Some("Not Tried Yet"),
        "overlooked" => Some("Overlooked"),
        _ => None,
    }
}

fn validate_event(event: &Value) -> Result<(String, String), String> {
    let fields = object(event, "decision event")?;
    only_keys(
        fields,
        &["id", "kind", "text", "evidence", "target"],
        "decision event",
    )?;
    let kind = string(
        fields
            .get("kind")
            .ok_or_else(|| err("decision event kind required"))?,
        "decision event kind",
    )?;
    if view_for(kind).is_none() && kind != "complete" {
        return Err(err("unknown decision event kind"));
    }
    let _text = string(
        fields
            .get("text")
            .ok_or_else(|| err("decision text required"))?,
        "decision text",
    )?;
    if let Some(evidence) = fields.get("evidence") {
        if !evidence.is_array() {
            return Err(err("decision evidence must be an array"));
        }
    }
    let id = match fields.get("id") {
        Some(value) => {
            let id = string(value, "decision event id")?;
            if id.trim().is_empty() {
                return Err(err("decision event id must be nonempty"));
            }
            id.to_owned()
        }
        None => digest(event)?,
    };
    if kind == "complete" {
        let target = string(
            fields
                .get("target")
                .ok_or_else(|| err("completion event target required"))?,
            "completion target",
        )?;
        if target.trim().is_empty() {
            return Err(err("completion target must be nonempty"));
        }
    }
    if kind == "failure"
        && fields
            .get("evidence")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(err("failure event requires evidence"));
    }
    Ok((kind.to_owned(), id))
}

/// Apply a batch atomically if the caller's revision is still current.
pub fn apply(current: &Value, expected_revision: u64, events: &[Value]) -> Result<Value, String> {
    let revision = validate_current(current)?;
    if revision != expected_revision {
        return Err(err("decision revision conflict"));
    }
    if events.is_empty() {
        return Ok(current.clone());
    }

    // Validate the whole batch, including completion targets, before cloning/mutating.
    let parsed = events
        .iter()
        .map(validate_event)
        .collect::<Result<Vec<_>, _>>()?;
    let mut batch_payloads = std::collections::BTreeMap::<String, &Value>::new();
    for ((_, id), event) in parsed.iter().zip(events) {
        if let Some(previous) = batch_payloads.insert(id.clone(), event) {
            if previous != event {
                return Err(err("decision event identity reused with different content"));
            }
        }
    }
    let existing: BTreeSet<&str> = current["event_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut known = existing;
    let mut working_ids: BTreeSet<String> = current["views"]["Working on"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    for ((_, id), event) in parsed.iter().zip(events) {
        if !known.insert(id.as_str()) {
            continue;
        }
        if event["kind"] == "work" {
            working_ids.insert(id.clone());
        }
        if event["kind"] == "complete" {
            let target = event["target"].as_str().unwrap();
            if !working_ids.contains(target) {
                return Err(err("completion target is not an existing Working on item"));
            }
        }
    }

    let mut next = current.clone();
    let mut seen: BTreeSet<String> = next["event_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    for ((kind, id), event) in parsed.into_iter().zip(events) {
        if !seen.insert(id.clone()) {
            continue;
        }
        let text = event["text"].clone();
        let evidence = event.get("evidence").cloned().unwrap_or_else(|| json!([]));
        if kind == "complete" {
            let target = event["target"].as_str().unwrap();
            for row in next["views"]["Working on"].as_array_mut().unwrap() {
                if row["id"] == target {
                    row["status"] = json!("completed");
                    row["completion"] = text.clone();
                    row["completion_evidence"] = evidence.clone();
                }
            }
        } else {
            let status = if kind == "work" { "active" } else { "recorded" };
            next["views"][view_for(&kind).unwrap()]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "id": id,
                    "text": text,
                    "evidence": evidence,
                    "status": status
                }));
        }
        next["event_ids"].as_array_mut().unwrap().push(json!(id));
    }
    next["revision"] = json!(
        revision
            .checked_add(1)
            .ok_or_else(|| err("decision revision overflow"))?
    );
    validate_current(&next)?;
    Ok(next)
}

fn canonical_view(name: &str) -> Option<&'static str> {
    match name {
        "Working on" | "working_on" => Some("Working on"),
        "Failed Before" | "failed_before" => Some("Failed Before"),
        "Not Tried Yet" | "not_tried_yet" => Some("Not Tried Yet"),
        "Overlooked" | "overlooked" => Some("Overlooked"),
        _ => None,
    }
}

/// Replace only the supplied views from a trusted Editor operation, with CAS.
/// Editor status cannot assert that proposed work was completed.
pub fn from_editor(current: &Value, expected_revision: u64, raw: &Value) -> Result<Value, String> {
    let revision = validate_current(current)?;
    if revision != expected_revision {
        return Err(err("decision revision conflict"));
    }
    let raw_fields = object(raw, "decision_tree")?;
    let views_value = if let Some(views) = raw_fields.get("views") {
        only_keys(raw_fields, &["views"], "decision_tree")?;
        views
    } else {
        raw
    };
    let views = object(views_value, "decision views")?;
    let mut replacements: Vec<(String, Vec<Value>)> = Vec::new();
    let mut seen_views = BTreeSet::new();
    for (raw_name, value) in views {
        let name = canonical_view(raw_name)
            .ok_or_else(|| err("unknown decision view"))?
            .to_owned();
        if !seen_views.insert(name.clone()) {
            return Err(err("duplicate decision view alias"));
        }
        let values = if let Some(text) = value.as_str() {
            vec![json!({"text":text,"evidence":[]})]
        } else {
            value
                .as_array()
                .cloned()
                .ok_or_else(|| err("decision view entries must be arrays"))?
        };
        let mut rows = Vec::new();
        for item in values {
            let normalized = if let Some(text) = item.as_str() {
                json!({"text":text,"evidence":[]})
            } else {
                item
            };
            let fields = object(&normalized, "decision entry")?;
            only_keys(
                fields,
                &["id", "text", "evidence", "status"],
                "decision entry",
            )?;
            let text = string(
                fields
                    .get("text")
                    .ok_or_else(|| err("decision entries require text"))?,
                "decision entry text",
            )?;
            let evidence = fields.get("evidence").cloned().unwrap_or_else(|| json!([]));
            if !evidence.is_array() {
                return Err(err("decision entry evidence must be an array"));
            }
            let status = match fields.get("status") {
                Some(status) => string(status, "decision entry status")?,
                None if name == "Working on" => "active",
                None => "recorded",
            };
            if !["active", "recorded"].contains(&status) {
                return Err(err("Editor cannot assert completed work"));
            }
            let id = match fields.get("id") {
                Some(id) => {
                    let id = string(id, "decision entry id")?;
                    if id.trim().is_empty() {
                        return Err(err("decision id must be nonempty"));
                    }
                    id.to_owned()
                }
                None => digest(&json!([name, normalized]))?,
            };
            rows.push(json!({"id":id,"text":text,"evidence":evidence,"status":status}));
        }
        replacements.push((name, rows));
    }
    let mut next = current.clone();
    for (name, rows) in replacements {
        next["views"][name] = json!(rows);
    }
    next["revision"] = json!(
        revision
            .checked_add(1)
            .ok_or_else(|| err("decision revision overflow"))?
    );
    validate_current(&next)?;
    Ok(next)
}
