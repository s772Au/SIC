//! Immutable native context references, independent of editable text/PVI.
//! Payloads remain in their original carrier. A JSON digest authenticates the
//! reference block, never the remote URL/attachment's actual media bytes.
use crate::{
    error::{CoreError, Result},
    storage,
};
use serde_json::{Value, json};
use sqlx::SqliteConnection;
use std::collections::{BTreeMap, BTreeSet};

fn append(
    out: &mut Vec<Value>,
    value: &Value,
    anchor: Vec<Value>,
    row: &Value,
    kind: Option<&str>,
) -> Result<()> {
    out.push(json!({"native_anchor":anchor,"kind":kind.or_else(||value["type"].as_str()).unwrap_or("opaque"),
        "canonical_sha256":storage::digest(value)?,"digest_encoding":"native-block-canonical-json",
        "native_message_id":row["id"],"native_item_id":value["id"],
        "tool_call_id":value.get("tool_use_id").or_else(||row.get("tool_use_id")).or_else(||row.get("call_id")).or_else(||row.get("tool_call_id")).or_else(||row.get("toolCallId")),
        "immutable":true}));
    Ok(())
}

fn parts(
    out: &mut Vec<Value>,
    format: &str,
    values: &[Value],
    base: &[Value],
    row: &Value,
) -> Result<()> {
    for (index, part) in values.iter().enumerate() {
        let mut anchor = base.to_vec();
        anchor.push(json!(index));
        let text_kind = if format == "openai-responses-tool-output" {
            part["type"] == "input_text"
        } else if format == "openai-responses" {
            part["type"] == "input_text" || part["type"] == "output_text"
        } else {
            part["type"] == "text"
        };
        if text_kind && part["text"].is_string() {
            continue;
        }
        if format == "anthropic-messages" && part["type"] == "tool_result" {
            if let Some(nested) = part["content"].as_array() {
                anchor.push(json!("content"));
                parts(out, format, nested, &anchor, part)?;
            } else if !part["content"].is_string() {
                append(out, part, anchor, row, None)?;
            }
        } else {
            append(out, part, anchor, row, None)?;
        }
    }
    Ok(())
}

pub fn describe(format: &str, request: &Value) -> Result<Vec<Value>> {
    let key = match format {
        "openai-responses" => "input",
        "openai-chat" | "anthropic-messages" | "dsh-messages" => "messages",
        _ => return Err(CoreError::invalid("unsupported native carrier format")),
    };
    let mut out = Vec::new();
    if format == "openai-responses" && request[key].is_string() {
        return Ok(out);
    }
    let rows = request[key]
        .as_array()
        .ok_or_else(|| CoreError::invalid("native context array missing"))?;
    for (index, row) in rows.iter().enumerate() {
        let base = vec![json!(key), json!(index)];
        if format == "openai-responses" {
            match row["type"].as_str().unwrap_or("message") {
                "function_call_output" => {
                    if let Some(output) = row["output"].as_array() {
                        let mut anchor = base.clone();
                        anchor.push(json!("output"));
                        parts(
                            &mut out,
                            "openai-responses-tool-output",
                            output,
                            &anchor,
                            row,
                        )?;
                    } else if !row["output"].is_string() {
                        append(&mut out, row, base, row, None)?;
                    }
                    continue;
                }
                "message" => (),
                _ => {
                    append(&mut out, row, base, row, None)?;
                    continue;
                }
            }
        }
        if let Some(content) = row["content"].as_array() {
            let mut anchor = base.clone();
            anchor.push(json!("content"));
            parts(&mut out, format, content, &anchor, row)?;
        }
        if format == "openai-chat" {
            for field in ["audio", "reasoning_content", "reasoning"] {
                if row.get(field).is_some_and(|v| !v.is_null()) {
                    let mut anchor = base.clone();
                    anchor.push(json!(field));
                    append(&mut out, &row[field], anchor, row, Some(field))?;
                }
            }
            for (call_index, call) in row["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let mut anchor = base.clone();
                anchor.extend([json!("tool_calls"), json!(call_index)]);
                append(&mut out, call, anchor, row, Some("tool_call"))?;
            }
        }
    }
    Ok(out)
}

pub(crate) fn bind_origins(mut blocks: Vec<Value>, payload: &Value) -> Result<Vec<Value>> {
    let supplied = payload
        .get("host_opaque_origins")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let supplied = supplied
        .as_array()
        .ok_or_else(|| CoreError::invalid("host_opaque_origins must be an array"))?;
    let mut origins = BTreeSet::new();
    let mut anchors = BTreeSet::new();
    for text in payload["host_message_origins"]
        .as_array()
        .into_iter()
        .flatten()
    {
        origins.insert(storage::json_text(&json!([
            text["message_id"],
            text["block_index"]
        ]))?);
    }
    for origin in supplied {
        let message = origin["message_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::invalid("opaque message_id must be nonempty text"))?;
        let block = origin["block_index"]
            .as_u64()
            .ok_or_else(|| CoreError::invalid("opaque block_index must be nonnegative integer"))?;
        let descriptor = blocks
            .iter_mut()
            .find(|item| item["native_anchor"] == origin["native_anchor"])
            .ok_or_else(|| {
                CoreError::forbidden("opaque origin must map to an immutable native context block")
            })?;
        if !origins.insert(storage::json_text(&json!([message, block]))?)
            || !anchors.insert(storage::json_text(&descriptor["native_anchor"])?)
        {
            return Err(CoreError::forbidden(
                "opaque origins must map one-to-one and cannot alias text origins",
            ));
        }
        descriptor["host_origin"] = json!({"message_id":message,"block_index":block});
    }
    Ok(blocks)
}

pub(crate) fn source_blocks(source: &Value) -> Result<Vec<Value>> {
    if let Some(blocks) = source["opaque_blocks"].as_array() {
        return Ok(blocks.clone());
    }
    // Upgrade old archives in the read projection; never rewrite immutable sources.
    if let Some(request) = source
        .get("sidecar")
        .or_else(|| source.get("request"))
        .filter(|request| request.is_object())
    {
        return describe(source["format"].as_str().unwrap_or("openai-chat"), request);
    }
    Ok(Vec::new())
}

fn occurrence_identity(block: &Value, format: &Value) -> Result<Option<String>> {
    let origin = &block["host_origin"];
    let sdk = match (
        origin["message_id"].as_str(),
        origin["block_index"].as_u64(),
    ) {
        (Some(message), Some(index)) if !message.is_empty() => Some((message, index)),
        _ if format == "dsh-messages" => {
            let anchor = block["native_anchor"].as_array();
            match (block["native_message_id"].as_str(), anchor) {
                (Some(message), Some(path))
                    if !message.is_empty()
                        && path.len() == 4
                        && path[0] == "messages"
                        && path[2] == "content" =>
                {
                    path[3].as_u64().map(|index| (message, index))
                }
                _ => None,
            }
        }
        _ => None,
    };
    sdk.map(|(message, index)| {
        storage::json_text(&json!([message, index, block["canonical_sha256"]]))
    })
    .transpose()
}

pub(crate) async fn from_sources(
    conn: &mut SqliteConnection,
    source_ids: &[String],
) -> Result<Vec<Value>> {
    let mut pending: Vec<_> = source_ids
        .iter()
        .rev()
        .map(|id| (id.clone(), "source"))
        .collect();
    let mut visited = BTreeSet::new();
    let mut out: Vec<Value> = Vec::new();
    let mut identities = BTreeMap::<String, usize>::new();
    while let Some((id, kind)) = pending.pop() {
        if !visited.insert((id.clone(), kind)) {
            continue;
        }
        let value = storage::get_object(conn, &id, Some(kind)).await?;
        if kind == "source" {
            for mut block in source_blocks(&value)? {
                let identity = occurrence_identity(&block, &value["format"])?;
                if let Some(position) = identity.as_ref().and_then(|key| identities.get(key)) {
                    let observations = out[*position]["observed_source_ids"]
                        .as_array_mut()
                        .expect("created observation list");
                    if !observations.contains(&json!(id)) {
                        observations.push(json!(id));
                    }
                    continue;
                }
                block["source_id"] = json!(id);
                block["observed_source_ids"] = json!([id]);
                block["format"] = value["format"].clone();
                block["representation"] = json!("immutable-native");
                if let Some(identity) = identity {
                    identities.insert(identity, out.len());
                }
                out.push(block);
            }
            for dependency in value["dependencies"].as_array().into_iter().flatten() {
                if let Some(id) = dependency.as_str() {
                    pending.push((id.to_owned(), "source"));
                }
            }
            // Membership is coverage; cb_dependencies can also include an
            // entire prior protected cut used only for authorization. Its
            // ACL/revocation is still checked by get_object, but it must not
            // be repeatedly expanded as newly covered media history.
            for member in value["members"].as_array().into_iter().flatten() {
                if let Some(id) = member.as_str() {
                    pending.push((id.to_owned(), "cb"));
                }
            }
            if value["origin"]["kind"] == "volatile-continuation" {
                if let Some(id) = value["origin"]["cb_id"].as_str() {
                    pending.push((id.to_owned(), "cb"));
                }
            }
        } else {
            for source in value["source_ids"].as_array().into_iter().flatten() {
                if let Some(id) = source.as_str() {
                    pending.push((id.to_owned(), "source"));
                }
            }
            for member in value["members"].as_array().into_iter().flatten() {
                if let Some(id) = member.as_str() {
                    pending.push((id.to_owned(), "cb"));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relocated_origin_collapses_copies_and_preserves_offload_snapshots() {
        let store = storage::Store::memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        for (id, origin, offloaded) in [
            ("first", "original-message", false),
            ("relocated", "original-message", false),
            ("offloaded", "original-message", true),
            ("unrelated", "another-message", false),
        ] {
            let request = json!({"messages":[{"id":id,"role":"user","content":[
                {"type":"image","attachment":{"attachmentId":"same-attachment"},"offloaded":offloaded}
            ]}]});
            let payload = json!({"host_opaque_origins":[
                {"message_id":origin,"block_index":8,"native_anchor":["messages",0,"content",0]}
            ]});
            let opaque_blocks =
                bind_origins(describe("dsh-messages", &request).unwrap(), &payload).unwrap();
            storage::put_object(
                &mut tx,
                "source",
                id,
                &json!({
                    "format":"dsh-messages","sidecar":request,"opaque_blocks":opaque_blocks
                }),
            )
            .await
            .unwrap();
        }
        let ids = ["first", "relocated", "offloaded", "unrelated"].map(String::from);
        let view = from_sources(&mut tx, &ids).await.unwrap();
        assert_eq!(view.len(), 3);
        assert_eq!(
            view[0]["host_origin"],
            json!({"message_id":"original-message","block_index":8})
        );
        assert_eq!(
            view[0]["observed_source_ids"],
            json!(["first", "relocated"])
        );
        assert_ne!(view[0]["canonical_sha256"], view[1]["canonical_sha256"]);
        assert_eq!(view[1]["source_id"], "offloaded");
        assert_eq!(view[0]["canonical_sha256"], view[2]["canonical_sha256"]);
        assert_eq!(view[2]["source_id"], "unrelated");
    }

    #[tokio::test]
    async fn old_archive_opaque_manifest_is_derived_without_rewriting_and_checks_revocation() {
        let store = storage::Store::memory().await.unwrap();
        let mut tx = store.begin().await.unwrap();
        let old = json!({"format":"dsh-messages","sidecar":{"messages":[{"id":"legacy-message","role":"user","content":[{"type":"image","source":{"url":"https://media.invalid/legacy.png"}}]}]},"units":[],"dependencies":[],"cb_dependencies":[]});
        storage::put_object(&mut tx, "source", "old-source", &old)
            .await
            .unwrap();
        storage::put_object(
            &mut tx,
            "cb",
            "old-cb",
            &json!({"source_ids":["old-source"],"members":[],"body":"legacy summary"}),
        )
        .await
        .unwrap();
        storage::put_object(&mut tx,"source","higher-source",&json!({"format":"openai-chat","sidecar":{"messages":[{"role":"user","content":"legacy summary"}]},"cb_dependencies":["old-cb"],"members":["old-cb"]})).await.unwrap();
        let rows = from_sources(&mut tx, &["higher-source".into()])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source_id"], "old-source");
        assert_eq!(rows[0]["native_message_id"], "legacy-message");
        assert_eq!(
            storage::get_object(&mut tx, "old-source", Some("source"))
                .await
                .unwrap(),
            old
        );
        sqlx::query("INSERT INTO revocations(id,record) VALUES('old-source','{}')")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            from_sources(&mut tx, &["higher-source".into()])
                .await
                .unwrap_err()
                .status,
            403
        );
    }
    #[tokio::test]
    async fn same_sdk_origin_reuses_reference_but_without_identity_equal_content_is_not_merged() {
        for identified in [false, true] {
            let store = storage::Store::memory().await.unwrap();
            let mut tx = store.begin().await.unwrap();
            let mut ids = Vec::new();
            for round in 0..24 {
                let id = format!("source-{round}");
                let mut raw = json!({"messages":[{"role":"user","content":[{"type":"image","source":{"url":"https://media.invalid/same.png"}}]}]});
                if identified {
                    raw["messages"][0]["id"] = json!("same-sdk-message");
                }
                storage::put_object(
                    &mut tx,
                    "source",
                    &id,
                    &json!({"format":"dsh-messages","sidecar":raw,"units":[]}),
                )
                .await
                .unwrap();
                ids.push(id);
                let view = from_sources(&mut tx, &ids).await.unwrap();
                assert_eq!(view.len(), if identified { 1 } else { round + 1 });
            }
            let view = from_sources(&mut tx, &ids).await.unwrap();
            if identified {
                assert_eq!(view[0]["observed_source_ids"].as_array().unwrap().len(), 24);
            }
        }
    }
}
