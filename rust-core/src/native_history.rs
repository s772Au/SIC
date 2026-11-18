//! Bounded, read-only provenance for native transactions. Reads use the same
//! dependency/revocation checks as source and CB reads.
use crate::{
    core::{Core, OperationContext},
    error::{CoreError, Result},
    storage,
};
use serde_json::{Value, json};

pub(crate) async fn read(core: &Core, auth: &OperationContext, payload: &Value) -> Result<Value> {
    let source_id = payload["source_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::invalid("source_id must be nonempty text"))?;
    let after = match payload.get("after_version") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or_else(|| CoreError::invalid("after_version must be a nonnegative integer"))?,
        ),
    };
    let limit = match payload.get("limit") {
        None => 20,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=100).contains(n))
            .ok_or_else(|| CoreError::invalid("limit must be an integer from 1 to 100"))?,
    };
    let after_i = match payload.get("after_i_revision") {
        None => 0,
        Some(value) => value.as_u64().filter(|n| *n <= i64::MAX as u64)
            .ok_or_else(|| CoreError::invalid("after_i_revision must be a nonnegative integer"))?,
    };
    let mut tx = core.store.begin().await?;
    let source = storage::get_object(&mut tx, source_id, Some("native_source")).await?;
    if source["actor"].as_str() != Some(auth.actor.as_str())
        || source.get("chain_id").and_then(Value::as_str) != auth.chain_id.as_deref()
    {
        return Err(CoreError::forbidden(
            "native history outside actor or chain",
        ));
    }
    if let Some(chain) = &auth.chain_id {
        storage::get_chain(&mut tx, chain, &auth.actor).await?;
    }
    core.authorize_native_projection(&mut tx, auth, &source)
        .await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM objects WHERE kind='native_version' AND json_extract(data,'$.source_id')=? AND CAST(json_extract(data,'$.version') AS INTEGER)>? ORDER BY CAST(json_extract(data,'$.version') AS INTEGER) LIMIT ?"
    ).bind(source_id).bind(after.map(|n| n as i64).unwrap_or(-1)).bind((limit + 1) as i64)
        .fetch_all(&mut *tx).await?;
    let has_more = ids.len() > limit as usize;
    let mut versions = Vec::new();
    for id in ids.iter().take(limit as usize) {
        let mut version = storage::get_object(&mut tx, id, Some("native_version")).await?;
        version["object_id"] = json!(id);
        versions.push(version);
    }
    let publication_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM objects WHERE kind='native_publication' AND json_extract(data,'$.source_id')=? LIMIT 1"
    ).bind(source_id).fetch_optional(&mut *tx).await?;
    let publication = match publication_id {
        Some(id) => storage::get_object(&mut tx, &id, Some("native_publication")).await?,
        None => Value::Null,
    };
    let projection_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM objects WHERE kind='native_i_projection' AND json_extract(data,'$.source_id')=? AND CAST(json_extract(data,'$.revision') AS INTEGER)>? ORDER BY CAST(json_extract(data,'$.revision') AS INTEGER) LIMIT ?"
    ).bind(source_id).bind(after_i as i64).bind((limit + 1) as i64)
        .fetch_all(&mut *tx).await?;
    let has_more_i = projection_ids.len() > limit as usize;
    let mut i_projections = Vec::new();
    for id in projection_ids.iter().take(limit as usize) {
        let projection = storage::get_object(&mut tx, id, Some("native_i_projection")).await?;
        core.authorize_native_projection(&mut tx, auth, &projection).await?;
        i_projections.push(projection);
    }
    let next_i = if has_more_i { i_projections.last().map(|p| p["revision"].clone()).unwrap_or(Value::Null) } else { Value::Null };
    let next = if has_more {
        versions
            .last()
            .map(|v| v["version"].clone())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let opaque_blocks = crate::native_opaque::source_blocks(&source)?;
    tx.commit().await?;
    Ok(
        json!({"ok":true,"operation":"native.history","context":{"actor":auth.actor,"chain_id":auth.chain_id},
        "evidence":{"provider_called":false},"result":{"source_id":source_id,"source":source,
        "opaque_blocks":opaque_blocks,"versions":versions,"publication":publication,"has_more":has_more,"next_after_version":next,
        "i_projections":i_projections,"has_more_i":has_more_i,"next_after_i_revision":next_i}}),
    )
}
