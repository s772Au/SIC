//! Model-visible navigation through the published Worker representation.
use super::*;

pub(super) fn original_message(source: &Value, unit: &Value) -> Value {
    if source["format"] != "dsh-messages" {
        return Value::Null;
    }
    let Some(path) = unit["anchor"]["native_path"].as_array() else {
        return Value::Null;
    };
    let [messages, message_index, content, block_index, text] = path.as_slice() else {
        return Value::Null;
    };
    if messages != "messages" || content != "content" || text != "text" {
        return Value::Null;
    }
    let Some(index) = message_index.as_u64().and_then(|n| usize::try_from(n).ok()) else {
        return Value::Null;
    };
    let Some(message) = source["sidecar"]["messages"].get(index) else {
        return Value::Null;
    };
    let mut origin = json!({"message_id":message["id"], "block_index":block_index, "role":message["role"]});
    if message["role"] == "tool" {
        origin["tool_call_id"] = message["toolCallId"].clone();
        origin["is_error"] = message["isError"].clone();
    }
    origin
}

impl Core {
    pub(super) async fn archive_expand(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        args: &Value,
    ) -> Result<Value> {
        if obj(args)?.keys().any(|key| !["cb_id", "wb_id", "cut_id", "offset", "limit"].contains(&key.as_str())) {
            return Err(CoreError::invalid("unknown archive navigation argument"));
        }
        let (offset, limit) = Self::worker_page_bounds(args, 4, 8)?;
        let view = agent.role_views.get("worker")
            .ok_or_else(|| CoreError::invalid("Worker view missing"))?;
        let context = agent.contexts.get(&view.context_id)
            .ok_or_else(|| CoreError::invalid("Worker context missing"))?;
        let mut verified = storage::VerifiedObjects::in_archive_scope(agent.board_id.as_deref());
        let (cb, wb) = match (args.get("cb_id"), args.get("wb_id")) {
            (None, None) => {
                let cut = required_str(args, "cut_id")
                    .map_err(|_| CoreError::invalid("specify one cb_id, wb_id or cut_id"))?;
                let (context_id, ids) = self.archive_directory(conn, agent, cut, &mut verified).await?;
                let mut blocks = Vec::new();
                for (index, id) in ids.iter().enumerate().skip(offset).take(limit) {
                    let block = verified.read(conn, id, Some("cb")).await?;
                    blocks.push(json!({"ordinal":index+1,"cb_id":id,"wb_id":block["wb_id"],"level":block["level"]}));
                }
                let next = offset.saturating_add(blocks.len());
                return Ok(json!({"cut_id":cut,"context_id":context_id,"active_blocks":blocks,
                    "total":ids.len(),"offset":offset,"next_offset":(next < ids.len()).then_some(next)}));
            }
            (Some(_), None) if args.get("cut_id").is_none() => (Some(required_str(args, "cb_id")?), None),
            (None, Some(_)) if args.get("cut_id").is_none() => (None, Some(required_str(args, "wb_id")?)),
            _ => return Err(CoreError::invalid("specify one cb_id, wb_id or cut_id")),
        };
        for (order, level) in &context.levels {
            let pair = level.pairs.iter().find(|(wid, cid)| {
                cb == Some(cid.as_str()) || wb == Some(wid.as_str())
            });
            if let Some((wb_id, cb_id)) = pair {
                let block = verified.read(conn, cb_id, Some("cb")).await?;
                let working = verified.read(conn, wb_id, Some("wb")).await?;
                if block["wb_id"] != wb_id.as_str() || !level.cbs.contains(cb_id) || !level.wbs.contains(wb_id) {
                    return Err(CoreError::conflict("published WB/CB relationship differs"));
                }
                return self.archive_block_page(conn, &mut verified, *order, &block, &working, offset, limit).await;
            }
        }
        Err(CoreError::forbidden("block is outside the Worker published archive"))
    }

    async fn archive_directory(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        wanted: &str,
        verified: &mut storage::VerifiedObjects,
    ) -> Result<(String, Vec<String>)> {
        // The Worker may publish newer material between two directory pages.
        // Prove ancestry before using an old view and check its current ACL.
        let mut ancestry = storage::VerifiedObjects::default();
        let mut cursor = agent.current_cut.clone();
        let mut seen = BTreeSet::new();
        let cut = loop {
            let id = cursor.ok_or_else(|| CoreError::forbidden("archive directory is not on this chain's ancestry"))?;
            if !seen.insert(id.clone()) {
                return Err(CoreError::conflict("cut ancestry cycle"));
            }
            if id == wanted {
                break verified.read(conn, &id, Some("cut")).await?;
            }
            let previous = ancestry.read(conn, &id, Some("cut")).await?;
            cursor = previous["previous_cut"].as_str().map(str::to_owned);
        };
        let view: View = serde_json::from_value(cut["role_views"]["worker"].clone())?;
        if view.context_id != self.worker_context(agent)? {
            return Err(CoreError::forbidden("archive directory is outside the current Worker context"));
        }
        let mut snapshot = agent.clone();
        snapshot.contexts = serde_json::from_value(cut["contexts"].clone())?;
        snapshot.groups = serde_json::from_value(cut["groups"].clone())?;
        let ids = ids_for_view_snapshot(conn, &snapshot, &view.context_id, view.level, verified).await?;
        Ok((view.context_id, ids))
    }

    async fn archive_block_page(
        &self,
        conn: &mut SqliteConnection,
        verified: &mut storage::VerifiedObjects,
        level: u32,
        block: &Value,
        working: &Value,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        let frozen = working["source_ids"].as_array().ok_or_else(|| CoreError::invalid("WB source IDs missing"))?;
        let sources = block["source_ids"].as_array().ok_or_else(|| CoreError::invalid("CB source IDs missing"))?;
        if !sources.starts_with(frozen.as_slice()) {
            return Err(CoreError::conflict("published materials differ from the frozen WB"));
        }
        let members = working["members"].as_array().ok_or_else(|| CoreError::invalid("WB members missing"))?;
        let children: Vec<_> = members.iter().skip(offset).take(limit)
            .map(|member| json!({"cb_id":member,"level":level-1}))
            .collect();
        let mut units = Vec::new();
        let mut unit_count = 0;
        for (source_index, id) in sources.iter().enumerate() {
            let id = id.as_str().ok_or_else(|| CoreError::invalid("invalid archived source ID"))?;
            let source = verified.read(conn, id, Some("source")).await?;
            let originals = source["units"].as_array().ok_or_else(|| CoreError::invalid("source units missing"))?;
            for unit in originals.iter().skip(offset.saturating_sub(unit_count)).take(limit - units.len()) {
                let unit_id = required_str(unit, "unit_id")?;
                let text = unit["text"].as_str().ok_or_else(|| CoreError::invalid("source unit text missing"))?;
                let materialized = if source_index == 0 { unit_id.to_owned() } else { format!("{id}:{unit_id}") };
                let lower_cb = unit["anchor"]["cb_id"].as_str();
                units.push(json!({"source_id":id,"unit_id":unit_id,"materialized_unit_id":materialized,
                    "frozen_in_wb":source_index < frozen.len(),"lower_cb_id":lower_cb,
                    "source_page_available":level==1,"original_message":original_message(&source,unit),
                    "preview":text.chars().take(160).collect::<String>(),"text_chars":text.chars().count()}));
            }
            unit_count += originals.len();
        }
        let next = offset.saturating_add(limit);
        Ok(json!({"context_id":block["context_id"],"cb_id":block["cb_id"],"wb_id":block["wb_id"],
            "level":level,"members":children,"member_count":members.len(),"source_units":units,"unit_count":unit_count,
            "offset":offset,"next_offset":(next < members.len().max(unit_count)).then_some(next)}))
    }

    pub(super) async fn archive_ops(
        &self,
        conn: &mut SqliteConnection,
        agent: &Agent,
        args: &Value,
    ) -> Result<Value> {
        if obj(args)?.keys().any(|key| !["kind", "object_id", "offset", "limit"].contains(&key.as_str()))
            || required_str(args, "kind")? != "ops"
        {
            return Err(CoreError::invalid("published operation pages require kind ops"));
        }
        let id = required_str(args, "object_id")?;
        let (offset, limit) = Self::worker_page_bounds(args, 2048, 8192)?;
        self.authorize_worker_memory(conn, agent, "ops", id).await?;
        let mut verified = storage::VerifiedObjects::in_archive_scope(agent.board_id.as_deref());
        let block = verified.read(conn, id, Some("cb")).await?;
        let ops = block["ops"].as_array().ok_or_else(|| CoreError::invalid("CB operations missing"))?;
        let encoded = storage::json_text(&block["ops"])?;
        let total = encoded.chars().count();
        let value: String = encoded.chars().skip(offset).take(limit).collect();
        let next = offset.saturating_add(value.chars().count());
        Ok(json!({"kind":"ops","object_id":id,"wb_id":block["wb_id"],"value":value,"operation_count":ops.len(),
            "offset":offset,"total":total,"next_offset":(next < total).then_some(next),
            "unit":"unicode-codepoints","full_digest":storage::sha256(encoded.as_bytes())}))
    }
}
