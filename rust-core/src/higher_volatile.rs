//! Bind immutable higher-level V sources to a real same-level next WB.
use super::*;
impl Core {
    pub(super) async fn validate_higher_volatile(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        ctx: &str,
        level: u32,
        ids: &[String],
    ) -> Result<Vec<String>> {
        let pending = agent
            .contexts
            .get(ctx)
            .and_then(|context| context.levels.get(&level))
            .map(|row| row.pending.clone())
            .unwrap_or_default();
        if ids.iter().any(|id| id.is_empty())
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
            || !pending.starts_with(ids)
        {
            return Err(CoreError::conflict(
                "volatile binding must name the exact earliest distinct pending source prefix",
            ));
        }
        if !ids.is_empty() && pending.len() != ids.len() {
            return Err(CoreError::conflict(
                "an admitted WB follows volatile sources; it cannot be bypassed by regrouping",
            ));
        }
        let mut legacy = Vec::new();
        for id in ids {
            if agent
                .groups
                .get(id)
                .is_none_or(|parts| parts != &vec![id.clone()])
            {
                return Err(CoreError::conflict(
                    "volatile source is already grouped or outside this chain",
                ));
            }
            let source = storage::get_object(conn, id, Some("source")).await?;
            if source["origin"]["kind"] != "volatile-continuation" {
                return Err(CoreError::forbidden(
                    "binding requires an actual Core volatile source",
                ));
            }
            let cb =
                storage::get_object(conn, required_str(&source["origin"], "cb_id")?, Some("cb"))
                    .await?;
            if cb["context_id"] != ctx || cb["level"] != level {
                return Err(CoreError::forbidden(
                    "volatile source belongs to another context or level",
                ));
            }
            if let Some(row) = agent
                .contexts
                .get(ctx)
                .and_then(|context| context.levels.get(&level))
            {
                for wid in &row.wbs {
                    if row.superseded_wbs.contains_key(wid) {
                        continue;
                    }
                    let wb = storage::get_object(conn, wid, Some("wb")).await?;
                    if wb["source_id"] != *id {
                        continue;
                    }
                    if wb["reason"] != "volatile-continuation"
                        || row.pairs.contains_key(wid)
                        || agent.jobs.values().any(|job| job.wb_id == *wid)
                    {
                        return Err(CoreError::conflict(
                            "volatile source already has an admitted Editor WB; explicit recovery required",
                        ));
                    }
                    if wb["source_ids"] != json!([id]) {
                        return Err(CoreError::conflict(
                            "legacy volatile source membership changed",
                        ));
                    }
                    legacy.push(wid.clone());
                }
            }
        }
        if ids.is_empty() {
            if let Some(first) = pending.first() {
                let source = storage::get_object(conn, first, Some("source")).await?;
                if source["origin"]["kind"] == "volatile-continuation" {
                    return Err(CoreError::conflict(
                        "earliest volatile material requires an explicit same-level binding",
                    ));
                }
            }
        }
        Ok(legacy)
    }
}

// Materializer-owned ordinal mapping. No text search grants a source relation.
pub(super) fn make_derivations(source: &Value, index: &Value, owner: &str) -> Result<Value> {
    let mut result = Vec::new();
    for (position, unit) in source["units"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("volatile units missing"))?
        .iter()
        .enumerate()
    {
        let mut row=index.as_array().and_then(|rows|rows.get(position)).cloned().unwrap_or_else(||json!({"origin":"program-late-V","source_unit_ids":[],"source_anchors":[],"text_ranges":[]}));
        row["unit_id"] = unit["unit_id"].clone();
        row["owner_cb_id"] = json!(owner);
        row["output_sha256"] = json!(storage::sha256(
            unit["text"]
                .as_str()
                .ok_or_else(|| CoreError::invalid("volatile text missing"))?
                .as_bytes()
        ));
        result.push(row);
    }
    Ok(json!(result))
}

/// Reconstruct legacy derivation only from its archived Editor response ledger.
/// A free PVI/IPC V output stays explicitly unattributed.
pub(super) async fn derivations(
    conn: &mut SqliteConnection,
    agent: &Agent,
    source_id: &str,
) -> Result<Value> {
    let source = storage::get_object(conn, source_id, Some("source")).await?;
    if source["origin"]["kind"] != "volatile-continuation" {
        return Err(CoreError::invalid("source is not volatile"));
    }
    let owner = required_str(&source["origin"], "cb_id")?;
    let cb = storage::get_object(conn, owner, Some("cb")).await?;
    if let Some(records) = source["volatile_derivations"].as_array() {
        let units = source["units"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("volatile units missing"))?;
        if records.len() != units.len()
            || records.iter().zip(units).any(|(record, unit)| {
                record["unit_id"] != unit["unit_id"]
                    || record["owner_cb_id"] != owner
                    || record["output_sha256"]
                        != storage::sha256(unit["text"].as_str().unwrap_or_default().as_bytes())
            })
        {
            return Err(CoreError::conflict(
                "volatile derivation does not bind its immutable output",
            ));
        }
        return Ok(json!(records));
    }
    let mut assignments = Vec::new();
    let mut responses = Map::new();
    let editors = cb["editors"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("legacy volatile owner response ledger missing"))?;
    for editor in editors {
        let name = required_str(editor, "editor")?;
        assignments.push(json!({"editor":name,"unit_ids":editor["unit_ids"]}));
        let rid = editor["response_id"]
            .as_str()
            .or_else(|| {
                (editors.len() == 1)
                    .then(|| cb["original_response_id"].as_str())
                    .flatten()
            })
            .ok_or_else(|| CoreError::conflict("legacy volatile response identity unavailable"))?;
        responses.insert(
            name.into(),
            storage::get_object(conn, rid, Some("editor_response")).await?,
        );
    }
    let mut revision = cb["candidate_revision_id"].as_str().map(str::to_owned);
    let mut seen = BTreeSet::new();
    let mut revised = BTreeSet::new();
    while let Some(id) = revision {
        if !seen.insert(id.clone()) {
            return Err(CoreError::conflict("candidate revision cycle"));
        }
        let record = storage::get_object(conn, &id, Some("candidate_revision")).await?;
        let editor = required_str(&record, "editor")?;
        if revised.insert(editor.to_owned()) {
            responses.insert(editor.into(), record["candidate"].clone());
        }
        revision = record["previous_revision_id"].as_str().map(str::to_owned);
    }
    let ids: Vec<String> = serde_json::from_value(cb["source_ids"].clone())?;
    let snapshot =
        source_snapshot(conn, agent, required_str(&cb, "source_id")?, Some(&ids)).await?;
    let material = editing::materialize(
        &snapshot,
        &json!(assignments),
        &json!(responses),
        cb["context_id"] == "main" && cb["level"] == 1,
    )
    .map_err(err)?;
    let outputs = material["volatile"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("volatile material missing"))?;
    for (position, text) in outputs.iter().enumerate() {
        if source["units"][position]["text"] != *text {
            return Err(CoreError::conflict(
                "legacy volatile output differs from actual Editor materialization",
            ));
        }
    }
    make_derivations(&source, &material["volatile_index"], owner)
}

pub(super) async fn p_row_derivation(
    conn: &mut SqliteConnection,
    agent: &Agent,
    row: &Value,
) -> Result<Option<Value>> {
    let Some(id) = row["anchor"]["source_id"].as_str() else {
        return Ok(None);
    };
    let source = storage::get_object(conn, id, Some("source")).await?;
    if source["origin"]["kind"] != "volatile-continuation" {
        return Ok(None);
    }
    let units = source["units"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("volatile units missing"))?;
    let selected = if let Some(unit_id) = row["anchor"]["native"]["volatile_unit_id"].as_str() {
        units.iter().find(|unit| unit["unit_id"] == unit_id)
    } else {
        units
            .iter()
            .find(|unit| unit["anchor"] == row["anchor"]["native"])
    }
    .ok_or_else(|| {
        CoreError::conflict("P volatile anchor does not map to an immutable source unit")
    })?;
    let proofs = derivations(conn, agent, id).await?;
    Ok(proofs
        .as_array()
        .into_iter()
        .flatten()
        .find(|proof| proof["unit_id"] == selected["unit_id"])
        .cloned())
}

/// P metadata can follow a genuine V derivation, including multiple V rounds.
/// Each edge is anchored in a real immutable owner CB and actual materializer.
pub(super) async fn p_source_links(
    conn: &mut SqliteConnection,
    agent: &Agent,
    anchor: &Value,
) -> Result<Vec<Value>> {
    let mut stack = vec![(anchor.clone(), Vec::<String>::new())];
    let mut seen = BTreeSet::new();
    let mut links = Vec::new();
    while let Some((anchor, via)) = stack.pop() {
        if anchor["native"]["cb_id"].is_string() || anchor["native"]["host_origin"].is_object() {
            links.push(json!({"anchor":anchor,"via_cb_ids":via}));
            continue;
        }
        let Some(proof) = p_row_derivation(conn, agent, &json!({"anchor":anchor})).await? else {
            continue;
        };
        let identity = storage::digest(&json!([anchor["source_id"], proof["unit_id"]]))?;
        if !seen.insert(identity) {
            return Err(CoreError::conflict("volatile derivation cycle"));
        }
        let owner = required_str(&proof, "owner_cb_id")?;
        let owner_cb = storage::get_object(conn, owner, Some("cb")).await?;
        let mut next = via;
        next.push(owner.to_owned());
        for source_anchor in proof["source_anchors"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("volatile source anchors missing"))?
        {
            if let Some(child) = source_anchor["native"]["cb_id"].as_str() {
                if !owner_cb["members"]
                    .as_array()
                    .is_some_and(|members| members.iter().any(|member| member == child))
                {
                    return Err(CoreError::conflict(
                        "volatile source anchor points outside its actual owner WB",
                    ));
                }
            }
            stack.push((source_anchor.clone(), next.clone()));
        }
    }
    Ok(links)
}
