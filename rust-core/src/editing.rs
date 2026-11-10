//! Pure SIC content materialization and operation-prefix replay.
//!
//! This module deliberately knows nothing about persistence, model calls,
//! publication, permissions, or native tool execution. It only transforms a
//! frozen WB's assigned text units and returns content plus an auditable op
//! ledger. Protocol metadata on source units is carried through the index and
//! is never an editable target.

use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Number, Value};
use std::collections::{HashMap, HashSet};

const OPS: [&str; 7] = [
    "KEEP", "DELETE", "FLAG", "REORDER", "COMPRESS", "REPLACE", "ADD",
];
const OP_FIELDS: [&str; 9] = [
    "op",
    "target",
    "content",
    "reason",
    "order",
    "anchor",
    "lifecycle",
    "view_key",
    "risk",
];

struct StrictJson(Value);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> de::Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON value without duplicate object keys")
    }
    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::Bool(value)))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::Number(Number::from(value))))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::Number(Number::from(value))))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Number::from_f64(value)
            .map(|n| StrictJson(Value::Number(n)))
            .ok_or_else(|| E::custom("non-finite decision JSON constant"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::String(value.to_owned())))
    }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::String(value)))
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::Null))
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson(Value::Null))
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        StrictJson::deserialize(deserializer)
    }
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut rows = Vec::new();
        while let Some(item) = seq.next_element::<StrictJson>()? {
            rows.push(item.0);
        }
        Ok(StrictJson(Value::Array(rows)))
    }
    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut rows = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if rows.contains_key(&key) {
                return Err(de::Error::custom("duplicate decision JSON key"));
            }
            rows.insert(key, map.next_value::<StrictJson>()?.0);
        }
        Ok(StrictJson(Value::Object(rows)))
    }
}

fn validate_decision_tree(raw: &Value) -> Result<(), String> {
    let root = raw
        .as_object()
        .ok_or_else(|| invalid("decision_tree must be a four-view object"))?;
    let views = root.get("views").unwrap_or(raw);
    let views = views
        .as_object()
        .ok_or_else(|| invalid("decision views must be an object"))?;
    let canonical = ["Working on", "Failed Before", "Not Tried Yet", "Overlooked"];
    for key in views.keys() {
        let mapped = match key.as_str() {
            "working_on" => "Working on",
            "failed_before" => "Failed Before",
            "not_tried_yet" => "Not Tried Yet",
            "overlooked" => "Overlooked",
            value => value,
        };
        if !canonical.contains(&mapped) {
            return Err(invalid("unknown decision view"));
        }
    }
    for value in views.values() {
        let rows = match value {
            Value::String(_) => continue,
            Value::Array(rows) => rows,
            _ => return Err(invalid("decision view entries must be arrays")),
        };
        for row in rows {
            let item = match row {
                Value::String(_) => continue,
                Value::Object(item) => item,
                _ => return Err(invalid("decision entries require text and evidence array")),
            };
            if !item.get("text").is_some_and(Value::is_string)
                || item
                    .get("evidence")
                    .is_some_and(|evidence| !evidence.is_array())
            {
                return Err(invalid("decision entries require text and evidence array"));
            }
        }
    }
    Ok(())
}

fn validate_decision_operation(content: &str) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_str(content);
    let raw = StrictJson::deserialize(&mut deserializer)
        .map_err(|_| {
            invalid("decision_tree I operation content must encode a four-view JSON object")
        })?
        .0;
    deserializer.end().map_err(|_| {
        invalid("decision_tree I operation content must encode a four-view JSON object")
    })?;
    validate_decision_tree(&raw)?;
    Ok(raw)
}

#[derive(Clone)]
struct Unit {
    id: String,
    text: String,
    anchor: Value,
    continuation: bool,
    ordinal: usize,
}

#[derive(Clone)]
struct Addition {
    content: String,
    anchor: Option<(String, String)>,
}

fn invalid(message: &str) -> String {
    message.to_owned()
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| invalid(&format!("{name} must be an object")))
}

fn nonempty_string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(&format!("{name} must be a nonempty string")))
}

fn string_field<'a>(value: &'a Value, key: &str, name: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(&format!("{name} must be a string")))
}

fn integer(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
}

fn source_units(source: &Value) -> Result<Vec<Unit>, String> {
    let rows = source
        .get("units")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("source.units must be an array"))?;
    let mut seen = HashSet::new();
    let mut units = Vec::with_capacity(rows.len());
    for (ordinal, row) in rows.iter().enumerate() {
        object(row, "source unit")?;
        let id = nonempty_string(
            row.get("unit_id")
                .ok_or_else(|| invalid("source unit missing unit_id"))?,
            "unit_id",
        )?
        .to_owned();
        if !seen.insert(id.clone()) {
            return Err(invalid("duplicate source unit_id"));
        }
        let text = row
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("source unit text must be a string"))?
            .to_owned();
        let anchor = row
            .get("anchor")
            .cloned()
            .ok_or_else(|| invalid("source unit missing immutable anchor"))?;
        let continuation = match row.get("continuation") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(invalid("source unit continuation must be boolean")),
        };
        units.push(Unit {
            id,
            text,
            anchor,
            continuation,
            ordinal,
        });
    }
    Ok(units)
}

fn assignments(
    value: &Value,
    units: &[Unit],
    allow_overlapping: bool,
) -> Result<Vec<(String, Vec<String>)>, String> {
    let rows = value
        .as_array()
        .ok_or_else(|| invalid("assignments must be an array"))?;
    let valid: HashSet<&str> = units.iter().map(|unit| unit.id.as_str()).collect();
    let mut editors = HashSet::new();
    let mut assigned = HashSet::new();
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let fields = object(row, "assignment")?;
        if fields
            .keys()
            .any(|key| key != "editor" && key != "unit_ids")
        {
            return Err(invalid("unknown assignment field"));
        }
        let editor = nonempty_string(
            row.get("editor")
                .ok_or_else(|| invalid("assignment missing editor"))?,
            "editor",
        )?
        .to_owned();
        if !editors.insert(editor.clone()) {
            return Err(invalid("duplicate Editor assignment"));
        }
        let ids = row
            .get("unit_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("assignment unit_ids must be an array"))?;
        let mut scope = Vec::with_capacity(ids.len());
        for item in ids {
            let id = nonempty_string(item, "assigned unit_id")?.to_owned();
            if !valid.contains(id.as_str()) {
                return Err(invalid("assignment outside frozen WB scope"));
            }
            if !assigned.insert(id.clone()) && !allow_overlapping {
                return Err(invalid("overlapping Editor assignments"));
            }
            scope.push(id);
        }
        result.push((editor, scope));
    }
    Ok(result)
}

fn validate_response(
    scope: &HashSet<String>,
    response: &Value,
    primary_decision: bool,
) -> Result<(), String> {
    let fields = object(response, "Editor response")?;
    if fields.keys().any(|key| key != "ops" && key != "pvi") {
        return Err(invalid("unknown Editor response field"));
    }
    let ops = response
        .get("ops")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("ops must be an ordered array"))?;
    for op in ops {
        let fields = object(op, "operation")?;
        if fields.keys().any(|key| !OP_FIELDS.contains(&key.as_str())) {
            return Err(invalid(
                "unknown editing field; native protocol mutation forbidden",
            ));
        }
        let kind = string_field(op, "op", "operation op")?;
        if !OPS.contains(&kind) {
            return Err(invalid("unknown content operation"));
        }
        let lifecycle = match op.get("lifecycle") {
            None => "P",
            Some(Value::String(value)) => value.as_str(),
            Some(_) => return Err(invalid("content lifecycle must be P/V/I")),
        };
        if !["P", "V", "I"].contains(&lifecycle) {
            return Err(invalid("content lifecycle must be P/V/I"));
        }
        if lifecycle != "P" && !["ADD", "COMPRESS", "REPLACE", "FLAG"].contains(&kind) {
            return Err(invalid(
                "only generated content operations can route content to V/I",
            ));
        }
        if lifecycle == "I" && op.get("view_key").and_then(Value::as_str).is_none() {
            return Err(invalid("I operation requires view_key"));
        }
        if let Some(risk) = op.get("risk") {
            if !risk
                .as_str()
                .is_some_and(|r| ["none", "low", "medium", "high", "unknown"].contains(&r))
            {
                return Err(invalid("invalid loss-risk classification"));
            }
        }
        if kind == "ADD" {
            if op.get("target").is_some_and(|target| !target.is_null()) {
                return Err(invalid("ADD has no mutable source target"));
            }
        } else {
            let target = string_field(op, "target", "operation target")?;
            if !scope.contains(target) {
                return Err(invalid("operation outside assigned editable scope"));
            }
        }
        if let Some(anchor) = op.get("anchor") {
            let anchor_fields = object(anchor, "ADD anchor")?;
            if kind != "ADD"
                || anchor_fields.len() != 2
                || !anchor_fields.contains_key("target")
                || !anchor_fields.contains_key("position")
            {
                return Err(invalid(
                    "ADD anchor requires assigned target and before/after position",
                ));
            }
            let target = string_field(anchor, "target", "ADD anchor target")?;
            let position = string_field(anchor, "position", "ADD anchor position")?;
            if !scope.contains(target) || !["before", "after"].contains(&position) {
                return Err(invalid(
                    "ADD anchor requires assigned target and before/after position",
                ));
            }
        }
        if ["ADD", "REPLACE", "COMPRESS", "FLAG"].contains(&kind)
            && op.get("content").and_then(Value::as_str).is_none()
        {
            return Err(invalid("content string required"));
        }
        if primary_decision
            && lifecycle == "I"
            && op.get("view_key").and_then(Value::as_str) == Some("decision_tree")
        {
            validate_decision_operation(
                op.get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("content string required"))?,
            )?;
        }
        if kind == "REORDER" {
            let number = op.get("order").and_then(integer);
            if number.is_none() {
                return Err(invalid("REORDER requires integer order"));
            }
        }
    }

    let pvi = response
        .get("pvi")
        .ok_or_else(|| invalid("pvi accepts P/V/I only"))?;
    let pvi_fields = object(pvi, "pvi")?;
    if pvi_fields
        .keys()
        .any(|key| !["P", "V", "I"].contains(&key.as_str()))
    {
        return Err(invalid("pvi accepts P/V/I only"));
    }
    for key in ["P", "V"] {
        if let Some(value) = pvi.get(key) {
            let rows = value
                .as_array()
                .ok_or_else(|| invalid("P/V must be arrays of content strings"))?;
            if rows.iter().any(|row| !row.is_string()) {
                return Err(invalid("P/V must be arrays of content strings"));
            }
        }
    }
    if let Some(value) = pvi.get("I") {
        let views = object(value, "I")?;
        if primary_decision {
            if let Some(tree) = views.get("decision_tree") {
                validate_decision_tree(tree)?;
            }
        }
    }
    Ok(())
}

/// Apply assigned Editor responses to a frozen WB without touching its source.
///
/// `source` contains immutable units; `assignments` is an ordered array of
/// `{editor, unit_ids}`; `responses` maps those editor names to `{ops, pvi}`.
fn materialize_inner(
    source: &Value,
    assignments_value: &Value,
    responses: &Value,
    primary_decision: bool,
    allow_overlapping: bool,
) -> Result<Value, String> {
    let units = source_units(source)?;
    let assignments = assignments(assignments_value, &units, allow_overlapping)?;
    let response_map = object(responses, "responses")?;
    let expected: HashSet<&str> = assignments
        .iter()
        .map(|(editor, _)| editor.as_str())
        .collect();
    if response_map
        .keys()
        .any(|key| !expected.contains(key.as_str()))
    {
        return Err(invalid("response belongs to an unassigned Editor"));
    }

    let mut texts: HashMap<String, String> = units
        .iter()
        .map(|u| (u.id.clone(), u.text.clone()))
        .collect();
    let mut order: HashMap<String, i128> = units
        .iter()
        .map(|u| (u.id.clone(), u.ordinal as i128))
        .collect();
    let mut flags: HashMap<String, String> = HashMap::new();
    let mut persistent: Vec<String> = Vec::new();
    let mut volatile: Vec<String> = Vec::new();
    let mut instant = Map::new();
    let mut ops = Vec::new();
    let mut additions: Vec<Addition> = Vec::new();

    for (editor, scope_ids) in &assignments {
        let response = response_map
            .get(editor)
            .ok_or_else(|| invalid("missing Editor response"))?;
        let scope: HashSet<String> = scope_ids.iter().cloned().collect();
        validate_response(&scope, response, primary_decision)?;
        let response_ops = response["ops"].as_array().expect("validated ops");
        for raw in response_ops {
            let mut op = raw.clone();
            op.as_object_mut()
                .expect("validated operation object")
                .insert("editor".into(), Value::String(editor.clone()));
            let kind = op["op"].as_str().expect("validated op string").to_owned();
            let target = op.get("target").and_then(Value::as_str).map(str::to_owned);
            let lifecycle = op
                .get("lifecycle")
                .and_then(Value::as_str)
                .unwrap_or("P")
                .to_owned();
            ops.push(op.clone());

            if lifecycle != "P" {
                if ["REPLACE", "COMPRESS"].contains(&kind.as_str()) {
                    if let Some(target) = &target {
                        texts.remove(target);
                    }
                }
                let content = op
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if lifecycle == "V" {
                    volatile.push(content);
                } else {
                    let key = op
                        .get("view_key")
                        .and_then(Value::as_str)
                        .expect("validated I view_key")
                        .to_owned();
                    if instant.contains_key(&key) {
                        return Err(invalid("conflicting instantaneous content key"));
                    }
                    let value = if primary_decision && key == "decision_tree" {
                        validate_decision_operation(&content)?
                    } else {
                        Value::String(content)
                    };
                    instant.insert(key, value);
                }
                continue;
            }

            match kind.as_str() {
                "KEEP" => {}
                "DELETE" => {
                    if let Some(target) = &target {
                        texts.remove(target);
                    }
                }
                "REPLACE" | "COMPRESS" => {
                    texts.insert(
                        target.expect("targeted operation"),
                        op["content"]
                            .as_str()
                            .expect("validated content")
                            .to_owned(),
                    );
                }
                "FLAG" => {
                    let target = target.expect("targeted operation");
                    if !texts.contains_key(&target) {
                        return Err(invalid("cannot FLAG a deleted unit"));
                    }
                    let content = op["content"].as_str().expect("validated content");
                    let previous = flags.get(&target).cloned().unwrap_or_default();
                    let joined = if previous.is_empty() {
                        content.to_owned()
                    } else {
                        format!("{previous}; {content}")
                    };
                    flags.insert(
                        target,
                        joined.trim_matches(|ch| ch == ';' || ch == ' ').to_owned(),
                    );
                }
                "REORDER" => {
                    order.insert(
                        target.expect("targeted operation"),
                        integer(&op["order"]).expect("validated order"),
                    );
                }
                "ADD" => {
                    let anchor = op.get("anchor").and_then(Value::as_object).map(|a| {
                        (
                            a["target"]
                                .as_str()
                                .expect("validated anchor target")
                                .to_owned(),
                            a["position"]
                                .as_str()
                                .expect("validated anchor position")
                                .to_owned(),
                        )
                    });
                    additions.push(Addition {
                        content: op["content"]
                            .as_str()
                            .expect("validated content")
                            .to_owned(),
                        anchor,
                    });
                }
                _ => unreachable!("validated operation kind"),
            }
        }

        let pvi = &response["pvi"];
        for item in pvi.get("P").and_then(Value::as_array).into_iter().flatten() {
            let text = item.as_str().expect("validated P string").to_owned();
            persistent.push(text.clone());
            ops.push(json_add(&text, editor, "editor-P"));
        }
        volatile.extend(
            pvi.get("V")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|v| v.as_str().expect("validated V string").to_owned()),
        );
        if let Some(views) = pvi.get("I").and_then(Value::as_object) {
            for (key, value) in views {
                if instant.contains_key(key) {
                    return Err(invalid("Editors conflict on same instant view"));
                }
                instant.insert(key.clone(), value.clone());
            }
        }
    }

    let explicit: HashSet<String> = ops
        .iter()
        .filter_map(|op| op.get("target").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let keeps: Vec<String> = units
        .iter()
        .filter(|unit| !explicit.contains(&unit.id))
        .map(|unit| unit.id.clone())
        .collect();
    if !keeps.is_empty() {
        ops.push(serde_json::json!({"op":"KEEP","targets":keeps,"origin":"implicit-program"}));
    }

    let mut active: Vec<&Unit> = units
        .iter()
        .filter(|unit| texts.contains_key(&unit.id))
        .collect();
    active.sort_by_key(|unit| (order[&unit.id], unit.ordinal));
    for addition in &additions {
        if let Some((target, _)) = &addition.anchor {
            if !texts.contains_key(target) {
                return Err(invalid("ADD anchor was deleted from active view"));
            }
        }
    }
    let has_before = |unit_id: &str| {
        additions.iter().any(|a| {
            a.anchor
                .as_ref()
                .is_some_and(|(target, position)| target == unit_id && position == "before")
        })
    };
    let mut pieces: Vec<String> = Vec::new();
    for unit in active {
        for addition in additions.iter().filter(|a| {
            a.anchor
                .as_ref()
                .is_some_and(|(target, position)| target == &unit.id && position == "before")
        }) {
            pieces.push(addition.content.clone());
        }
        let mut rendered = texts[&unit.id].clone();
        if let Some(flag) = flags.get(&unit.id) {
            rendered.push_str("\n[FLAG] ");
            rendered.push_str(flag);
        }
        if unit.continuation && !pieces.is_empty() && !has_before(&unit.id) {
            pieces.last_mut().expect("nonempty").push_str(&rendered);
        } else {
            pieces.push(rendered);
        }
        for addition in additions.iter().filter(|a| {
            a.anchor
                .as_ref()
                .is_some_and(|(target, position)| target == &unit.id && position == "after")
        }) {
            pieces.push(addition.content.clone());
        }
    }
    pieces.extend(
        additions
            .iter()
            .filter(|a| a.anchor.is_none())
            .map(|a| a.content.clone()),
    );
    pieces.extend(persistent);
    let body = pieces.join("\n");

    let index = units.iter().map(|unit| {
        let operation_indexes = ops.iter().enumerate().filter_map(|(i, op)| {
            let directly_targets = op.get("target").and_then(Value::as_str) == Some(unit.id.as_str());
            let group_targets = op.get("targets").and_then(Value::as_array).is_some_and(|targets| targets.iter().any(|v| v.as_str() == Some(unit.id.as_str())));
            (directly_targets || group_targets).then_some(i)
        }).collect::<Vec<_>>();
        serde_json::json!({"unit_id":unit.id,"anchor":unit.anchor,"operation_indexes":operation_indexes})
    }).collect::<Vec<_>>();
    Ok(
        serde_json::json!({"body":body,"ops":ops,"volatile":volatile,"instant":Value::Object(instant),"index":index}),
    )
}

pub fn materialize(
    source: &Value,
    assignments_value: &Value,
    responses: &Value,
    primary_decision: bool,
) -> Result<Value, String> {
    materialize_inner(
        source,
        assignments_value,
        responses,
        primary_decision,
        false,
    )
}

fn json_add(content: &str, editor: &str, origin: &str) -> Value {
    serde_json::json!({"op":"ADD","content":content,"origin":origin,"editor":editor})
}

/// Rebuild a pure content state from the first `step` fixed operation records.
/// No model, tool, permission, publication, or external effect is replayed.
pub fn replay(
    source: &Value,
    records: &Value,
    step: usize,
    primary_decision: bool,
) -> Result<Value, String> {
    let rows = records
        .as_array()
        .ok_or_else(|| invalid("records must be an array"))?;
    if step > rows.len() {
        return Err(invalid("replay step must be 0..operation count"));
    }
    let units = source_units(source)?;
    let scope: Vec<Value> = units
        .iter()
        .map(|unit| Value::String(unit.id.clone()))
        .collect();
    let mut assignments = Vec::new();
    let mut responses = Map::new();
    let allowed: HashSet<&str> = OP_FIELDS.iter().copied().collect();
    let mut communications = Vec::new();
    for (i, record) in rows.iter().take(step).enumerate() {
        let fields = object(record, "replay operation")?;
        if record.get("op").and_then(Value::as_str) == Some("KEEP")
            && record.get("targets").is_some()
        {
            let targets = record["targets"]
                .as_array()
                .ok_or_else(|| invalid("replay KEEP targets must be an array"))?;
            let valid: HashSet<&str> = units.iter().map(|unit| unit.id.as_str()).collect();
            if targets
                .iter()
                .any(|target| target.as_str().is_none_or(|id| !valid.contains(id)))
            {
                return Err(invalid("replay KEEP has unknown WB target"));
            }
            continue;
        }
        if record.get("origin").and_then(Value::as_str) == Some("persistent-communication") {
            communications.push(
                record
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("persistent communication content must be a string"))?
                    .to_owned(),
            );
            continue;
        }
        let editor = format!("replay-{i}");
        assignments.push(serde_json::json!({"editor":editor,"unit_ids":scope}));
        if record.get("origin").and_then(Value::as_str) == Some("editor-P") {
            responses.insert(editor, serde_json::json!({"ops":[],"pvi":{"P":[record.get("content").and_then(Value::as_str).ok_or_else(|| invalid("editor-P content must be a string"))?]}}));
        } else {
            let mut op = Map::new();
            for (key, value) in fields {
                if allowed.contains(key.as_str()) {
                    op.insert(key.clone(), value.clone());
                }
            }
            responses.insert(
                editor.clone(),
                serde_json::json!({"ops":[Value::Object(op)],"pvi":{}}),
            );
        }
    }
    let mut result = materialize_inner(
        source,
        &Value::Array(assignments),
        &Value::Object(responses),
        primary_decision,
        true,
    )?;
    if !communications.is_empty() {
        let body = result
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let appended = communications.join("\n");
        result["body"] = Value::String(if body.is_empty() {
            appended
        } else {
            format!("{body}\n{appended}")
        });
    }
    Ok(serde_json::json!({
        "body":result["body"],
        "volatile":result["volatile"],
        "instant":result["instant"],
        "step":step,
        "operation_count":rows.len(),
        "external_effects_replayed":false,
        "basis":"archived WB with published ordered operation prefix"
    }))
}
