//! SDK origin coverage and actual surviving P are different facts.
use super::*;

fn key(origin: &Value) -> Result<Option<String>> {
    if origin["message_id"].as_str().is_none() || origin["block_index"].as_u64().is_none() {
        return Ok(None);
    }
    Ok(Some(storage::json_text(&json!([
        origin["message_id"],
        origin["block_index"]
    ]))?))
}

pub(super) async fn material_index(
    conn: &mut SqliteConnection,
    agent: &Agent,
    cb: &Value,
) -> Result<Value> {
    let index = &cb["index"];
    if index.as_array().is_some_and(|rows| {
        rows.iter()
            .all(|row| row["p_active"].is_boolean() && row["p_text_ranges"].is_array())
    }) {
        return Ok(index.clone());
    }
    // Old archives are immutable. Reconstruct their P bits with the actual
    // engine, never infer surviving content from coverage or string matching.
    let ids: Vec<String> = serde_json::from_value(cb["source_ids"].clone())?;
    let source = source_snapshot(conn, agent, required_str(cb, "source_id")?, Some(&ids)).await?;
    let ops = cb["ops"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("CB ops missing"))?;
    let material = editing::replay_with_index(
        &source,
        &cb["ops"],
        ops.len(),
        cb["context_id"] == "main" && cb["level"] == 1,
    )
    .map_err(err)?;
    if material["body"] != cb["body"] {
        return Err(CoreError::conflict("CB replay differs from published P"));
    }
    Ok(material["index"].clone())
}

pub(super) async fn bindings(
    conn: &mut SqliteConnection,
    agent: &Agent,
    root: &str,
    verified: &mut storage::VerifiedObjects,
) -> Result<Value> {
    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    let mut source_ids = BTreeSet::new();
    let mut positions = BTreeMap::<String, usize>::new();
    let mut origins = Vec::<Value>::new();
    let mut missing = Vec::new();
    let mut program_sources = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let cb = verified.read(conn, &id, Some("cb")).await?;
        let members = cb["members"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB members missing"))?;
        if !members.is_empty() {
            for member in members.iter().rev() {
                pending.push(
                    member
                        .as_str()
                        .ok_or_else(|| CoreError::invalid("CB member must be an ID"))?
                        .to_owned(),
                );
            }
        }
        for source_id in cb["source_ids"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB source identities missing"))?
        {
            let source_id = source_id
                .as_str()
                .ok_or_else(|| CoreError::invalid("source identity must be text"))?;
            if !source_ids.insert(source_id.to_owned()) {
                continue;
            }
            let source = verified.read(conn, source_id, Some("source")).await?;
            if source["origin"]["kind"] == "volatile-continuation" {
                let owner = required_str(&source["origin"], "cb_id")?;
                pending.push(owner.to_owned());
                program_sources.push(json!({"source_id":source_id,"kind":"volatile-continuation","owner_cb_id":owner}));
                continue;
            }
            let mut incomplete = false;
            for unit in source["units"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("source units missing"))?
            {
                if unit["anchor"]["cb_id"].is_string() {
                    continue;
                }
                let origin = &unit["anchor"]["host_origin"];
                let Some(key) = key(origin)? else {
                    incomplete = true;
                    continue;
                };
                if !positions.contains_key(&key) {
                    let mut origin = origin.clone();
                    origin["retained_in_p"] = json!(false);
                    origin["p_paths"] = json!([]);
                    positions.insert(key, origins.len());
                    origins.push(origin);
                }
            }
            if incomplete {
                missing.push(json!(source_id));
            }
        }
    }
    let mut pending = vec![(
        root.to_owned(),
        vec![root.to_owned()],
        Some(0_usize),
        Vec::<Value>::new(),
    )];
    let mut visited = BTreeSet::new();
    while let Some((id, path, frame, scope)) = pending.pop() {
        if !visited.insert(storage::digest(&json!([id, path, frame, scope]))?) {
            continue;
        }
        let cb = verified.read(conn, &id, Some("cb")).await?;
        let index = material_index(conn, agent, &cb).await?;
        for row in index
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB P index missing"))?
        {
            if row["p_active"] != true {
                continue;
            }
            let local = row["p_text_ranges"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("materialized P ranges missing"))?;
            let actual = if let Some(base) = frame {
                local.iter().map(|range|Ok(json!({"utf8_start":base+field_u64(range,"utf8_start")? as usize,"utf8_end":base+field_u64(range,"utf8_end")? as usize}))).collect::<Result<Vec<Value>>>()?
            } else {
                scope.clone()
            };
            let direct_child = row["anchor"]["native"]["cb_id"].as_str();
            let links = if let Some(child) = direct_child {
                if !cb["members"]
                    .as_array()
                    .is_some_and(|members| members.iter().any(|member| member == child))
                {
                    return Err(CoreError::conflict("P index points outside CB coverage"));
                }
                vec![json!({"anchor":row["anchor"],"via_cb_ids":[]})]
            } else if key(&row["anchor"]["native"]["host_origin"])?.is_some() {
                vec![json!({"anchor":row["anchor"],"via_cb_ids":[]})]
            } else {
                higher_volatile::p_source_links(conn, agent, &row["anchor"], verified).await?
            };
            for link in links {
                let mut next = path.clone();
                for via in link["via_cb_ids"].as_array().into_iter().flatten() {
                    next.push(
                        via.as_str()
                            .ok_or_else(|| {
                                CoreError::invalid("volatile owner CB identity missing")
                            })?
                            .to_owned(),
                    );
                }
                if let Some(child) = link["anchor"]["native"]["cb_id"].as_str() {
                    if next.iter().any(|item| item == child) {
                        return Err(CoreError::conflict("CB P ancestry cycle"));
                    }
                    let child_cb = verified.read(conn, child, Some("cb")).await?;
                    let child_frame = if direct_child.is_some() && local.len() == 1 {
                        let start = field_u64(&local[0], "utf8_start")? as usize;
                        let end = field_u64(&local[0], "utf8_end")? as usize;
                        if cb["body"].as_str().and_then(|body| body.get(start..end))
                            == child_cb["body"].as_str()
                        {
                            frame.map(|base| base + start)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    next.push(child.to_owned());
                    pending.push((child.to_owned(), next, child_frame, actual.clone()));
                } else if let Some(origin_key) = key(&link["anchor"]["native"]["host_origin"])? {
                    let position = positions.get(&origin_key).ok_or_else(|| {
                        CoreError::conflict("P origin missing from immutable source coverage")
                    })?;
                    let origin = &mut origins[*position];
                    origin["retained_in_p"] = json!(true);
                    origin["p_paths"]
                        .as_array_mut()
                        .ok_or_else(|| CoreError::invalid("P paths missing"))?
                        .push(json!(next));
                    if !origin["p_text_ranges"].is_array() {
                        origin["p_text_ranges"] = json!([]);
                        origin["p_spans"] = json!([]);
                    }
                    for range in &actual {
                        if !origin["p_text_ranges"]
                            .as_array()
                            .expect("initialized ranges")
                            .contains(range)
                        {
                            origin["p_text_ranges"]
                                .as_array_mut()
                                .expect("initialized ranges")
                                .push(range.clone());
                        }
                        let relation = if frame.is_some()
                            && link["via_cb_ids"]
                                .as_array()
                                .is_some_and(|ids| ids.is_empty())
                        {
                            "materialized_unit"
                        } else {
                            "derived_scope"
                        };
                        origin["p_spans"]
                            .as_array_mut()
                            .expect("initialized spans")
                            .push(json!({"cb_id":root,"range":range,"relation":relation}));
                    }
                }
            }
        }
    }
    Ok(
        json!({"cb_id":root,"origins":origins,"missing_source_ids":missing,"program_sources":program_sources,"p_basis":"actual materialized per-unit P index and immutable V derivations"}),
    )
}
