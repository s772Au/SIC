//! Verify actual P representation, separately from immutable CB coverage.
use super::*;
pub(crate) async fn query_collection_p_lineage(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain: &str,
    target: &str,
    wanted: Option<&str>,
) -> Result<Value> {
    let current = load_agent(tx, chain, actor).await?;
    let cut_id = wanted
        .map(str::to_owned)
        .or_else(|| current.current_cut.clone())
        .ok_or_else(|| {
            CoreError::forbidden("collection adoption requires a published selected cut")
        })?;
    // A historical snapshot must be an actual authorized ancestor, never a
    // caller-constructed context or an unrelated chain's cut.
    let mut cursor = current.current_cut.clone();
    let mut visited = BTreeSet::new();
    let mut verified = storage::VerifiedObjects::default();
    let cut = loop {
        let id = cursor
            .ok_or_else(|| CoreError::forbidden("adopted cut is not on this chain's ancestry"))?;
        if !visited.insert(id.clone()) {
            return Err(CoreError::conflict("cut ancestry cycle"));
        }
        let cut = verified.read(tx, &id, Some("cut")).await?;
        if id == cut_id {
            break cut;
        }
        cursor = cut["previous_cut"].as_str().map(str::to_owned);
    };
    if let Some(board) = cut["board_id"].as_str() {
        if Some(board) != current.board_id.as_deref() {
            return Err(CoreError::forbidden("adopted cut Board is no longer bound"));
        }
    }
    if let Some(board) = &current.board_id {
        BoardService::read_latest_i_in_transaction(
            tx,
            &BoardActor::from_authenticated_host(chain)?,
            board,
        )
        .await?;
    }
    let mut snapshot = current.clone();
    snapshot.contexts = serde_json::from_value(cut["contexts"].clone())?;
    snapshot.groups = serde_json::from_value(cut["groups"].clone())?;
    let view = &cut["role_views"]["worker"];
    let context = required_str(view, "context_id")?;
    let level = field_u64(view, "level")?;
    let level =
        u32::try_from(level).map_err(|_| CoreError::invalid("adopted view level invalid"))?;
    let selected = ids_for_view_snapshot(tx, &snapshot, context, level, &mut verified).await?;
    let mut stack: Vec<(String, Vec<String>)> = selected
        .iter()
        .map(|id| (id.clone(), vec![id.clone()]))
        .collect();
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    while let Some((id, path)) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let cb = verified.read(tx, &id, Some("cb")).await?;
        if id == target {
            paths.push(path);
            continue;
        }
        let members = cb["members"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB coverage missing"))?;
        let index = host_origins::material_index(tx, &snapshot, &cb).await?;
        for row in index
            .as_array()
            .ok_or_else(|| CoreError::invalid("CB materialized index missing"))?
        {
            if row["p_active"] != true {
                continue;
            }
            let direct = row["anchor"]["native"]["cb_id"].as_str();
            let links = if let Some(child) = direct {
                if !members.iter().any(|member| member == child) {
                    return Err(CoreError::conflict("P index points outside CB coverage"));
                }
                vec![json!({"anchor":row["anchor"],"via_cb_ids":[]})]
            } else {
                higher_volatile::p_source_links(tx, &snapshot, &row["anchor"], &mut verified).await?
            };
            for link in links {
                let Some(child) = link["anchor"]["native"]["cb_id"].as_str() else {
                    continue;
                };
                let mut next = path.clone();
                for via in link["via_cb_ids"].as_array().into_iter().flatten() {
                    next.push(required_str(&json!({"id":via}), "id")?.to_owned());
                }
                if next.iter().any(|id| id == child) {
                    return Err(CoreError::conflict("P lineage cycle"));
                }
                next.push(child.into());
                stack.push((child.into(), next));
            }
        }
    }
    Ok(
        json!({"eligible_current_p":!paths.is_empty(),"adopted_cut_id":cut_id,"selected_cb_ids":selected,"p_paths":paths,"basis":"actual materialized P index and immutable V derivations","historical_snapshot":current.current_cut.as_deref()!=Some(cut_id.as_str())}),
    )
}

pub(crate) async fn verify_collection_p_lineage(
    tx: &mut Transaction<'_, Sqlite>,
    actor: &str,
    chain: &str,
    target: &str,
    wanted: Option<&str>,
) -> Result<Value> {
    let proof = query_collection_p_lineage(tx, actor, chain, target, wanted).await?;
    if proof["eligible_current_p"] != true {
        return Err(CoreError::forbidden(
            "collection has no surviving P representation in the adopted cut",
        ));
    }
    Ok(proof)
}
