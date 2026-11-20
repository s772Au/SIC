//! Durable host-only native editing. Harness owns model admission/dispatch.
use crate::{
    core::{Core, OperationContext},
    error::{CoreError, Result},
    storage,
    transparent::{NativeContext, TextEdit},
};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::invalid(format!("{k} must be nonempty text")))
}
fn version(v: &Value, k: &str) -> Result<u64> {
    v[k].as_u64()
        .ok_or_else(|| CoreError::invalid(format!("{k} must be an unsigned integer")))
}
fn view(id: &str, c: &NativeContext, receipt: &Value) -> Result<Value> {
    let units: Vec<Value> = c
        .units()
        .iter()
        .map(|u| json!({"unit_id":u.unit_id,"text":u.text,"anchor":u.anchor}))
        .collect();
    Ok(
        json!({"source_id":id,"source_digest":c.source_digest(),"source_request":c.source_request(),"version":c.version(),"units":units,"request":c.request().map_err(CoreError::invalid)?,"receipt":receipt}),
    )
}
async fn commit(
    conn: &mut SqliteConnection,
    id: &str,
    c: &mut NativeContext,
    source_record: &Value,
    p: &Value,
) -> Result<Value> {
    let source = string(p, "expected_source_digest")?;
    let base = version(p, "expected_version")?;
    if source != c.source_digest() || base != c.version() {
        return Err(CoreError::conflict("native source or version conflict"));
    }
    let edits = p["edits"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("edits must be an array"))?;
    let edits: Vec<TextEdit> = edits
        .iter()
        .map(|e| {
            Ok(TextEdit {
                unit_id: string(e, "unit_id")?.into(),
                expected_text: e["expected_text"]
                    .as_str()
                    .ok_or_else(|| CoreError::invalid("expected_text must be text"))?
                    .into(),
                replacement: e["replacement"]
                    .as_str()
                    .ok_or_else(|| CoreError::invalid("replacement must be text"))?
                    .into(),
            })
        })
        .collect::<Result<_>>()?;
    let mut receipt = c
        .commit(source, base, &edits)
        .map_err(CoreError::conflict)?;
    if let Some(tool) = p.get("native_tool") {
        let name = string(tool, "name")?;
        let hash = string(tool, "arguments_sha256")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CoreError::invalid("arguments_sha256 must be SHA256 hex"));
        }
        receipt["native_tool"] = json!({"name":name,"arguments_sha256":hash});
    }
    if c.version() > i64::MAX as u64 {
        return Err(CoreError::conflict("native version exhausted"));
    }
    let delivery=p.get("delivery").cloned().unwrap_or(Value::Null);
    if !delivery.is_null() {
        if delivery.as_object().is_none_or(|object|object.keys().any(|key|key!="ops" && key!="pvi")) {
            return Err(CoreError::invalid("delivery must contain only ops and pvi"));
        }
        let original=NativeContext::new(string(source_record,"format")?,&source_record["request"]).map_err(CoreError::invalid)?;
        let editable:Vec<String>=serde_json::from_value(source_record["editable_unit_ids"].clone())?;
        let projection=crate::native_material::project(&original,&editable,&delivery["ops"],&delivery["pvi"])?;
        if c.units().iter().filter(|unit|editable.contains(&unit.unit_id)).any(|unit|
            projection["texts"][&unit.unit_id].as_str()!=Some(unit.text.as_str())) {
            return Err(CoreError::conflict("delivery operations do not reproduce this native candidate"));
        }
    }
    let delivery_digest=if delivery.is_null() {Value::Null} else {json!(storage::digest(&delivery)?)};
    let output = view(id, c, &receipt)?;
    storage::put_object(conn,"native_version",&storage::uid("native_version"),&json!({"source_id":id,"version":c.version(),"request":output["request"],"receipt":receipt,"delivery":delivery,"delivery_digest":delivery_digest})).await?;
    Ok(output)
}
async fn latest(conn: &mut SqliteConnection, verified: &mut storage::VerifiedObjects, id: &str) -> Result<Value> {
    // BEGIN IMMEDIATE covers latest-version read and successor insertion.
    let row = sqlx::query("SELECT id FROM objects WHERE kind='native_version' AND json_extract(data,'$.source_id')=? ORDER BY CAST(json_extract(data,'$.version') AS INTEGER) DESC LIMIT 1")
        .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(||CoreError::new("not_found","native version not found",404))?;
    let record: String = row.try_get("id")?;
    verified.read(conn, &record, Some("native_version")).await
}

pub(crate) async fn unpublished_predecessor(
    conn: &mut SqliteConnection,
    auth: &OperationContext,
    proof: &Value,
) -> Result<Value> {
    let id = string(proof, "source_id")?;
    let mut verified = storage::VerifiedObjects::default();
    let source = verified.read(conn, id, Some("native_source")).await?;
    if source["actor"].as_str() != Some(auth.actor.as_str())
        || source["chain_id"].as_str() != auth.chain_id.as_deref()
    {
        return Err(CoreError::forbidden("native predecessor belongs to another actor or chain"));
    }
    if source["source_digest"].as_str() != Some(string(proof, "expected_source_digest")?) {
        return Err(CoreError::conflict("native predecessor digest changed"));
    }
    if publication(conn, &mut verified, id).await?.is_some() || version(&latest(conn, &mut verified, id).await?, "version")? != 0 {
        return Err(CoreError::conflict("native predecessor already has a committed or published edit"));
    }
    Ok(source)
}

pub(crate) async fn operate(
    core: &Core,
    op: &str,
    auth: &OperationContext,
    p: &Value,
) -> Result<Value> {
    let mut tx = core.store.begin().await?;
    let mut verified = storage::VerifiedObjects::default();
    if op == "native.abandon" {
        let step = string(p, "native_step_id")?;
        let sources = p["sources"].as_array().filter(|sources| !sources.is_empty())
            .ok_or_else(|| CoreError::invalid("native release requires explicit source proofs"))?;
        let mut candidates = Vec::new();
        for proof in sources {
            if proof.as_object().is_none_or(|fields| fields.len() != 3) {
                return Err(CoreError::invalid("native release proof requires the id, digest and exact version"));
            }
            let id = string(proof, "source_id")?;
            let source = verified.read(&mut tx, id, Some("native_source")).await?;
            if source["actor"].as_str() != Some(auth.actor.as_str())
                || auth.chain_id.is_none() || source["chain_id"].as_str() != auth.chain_id.as_deref() {
                return Err(CoreError::forbidden("native release source belongs to another actor or chain"));
            }
            core.authorize_native_projection(&mut tx, auth, &source).await?;
            let current = latest(&mut tx, &mut verified, id).await?;
            if source["source_digest"].as_str() != Some(string(proof, "expected_source_digest")?)
                || source["native_step_id"].as_str() != Some(step)
                || version(&current, "version")? != version(proof, "expected_version")?
                || publication(&mut tx, &mut verified, id).await?.is_some() {
                return Err(CoreError::conflict("native release proof changed or the candidate is already published"));
            }
            candidates.push((id.to_owned(), source));
        }
        let fingerprint = storage::digest(&json!([auth.actor,auth.chain_id,p]))?;
        let key = format!("native-release:{fingerprint}");
        let prior: Option<String> = sqlx::query_scalar("SELECT result FROM receipts WHERE key=?")
            .bind(&key).fetch_optional(&mut *tx).await?;
        let replayed = prior.is_some();
        let result:Value = if let Some(prior) = prior { serde_json::from_str(&prior)? } else {
            for (id, _) in &candidates {
                reject_released_edit(&mut tx, id).await?;
            }
            let result = core.release_native_candidates(&mut tx, auth, &candidates).await?;
            let value = storage::json_text(&result)?;
            for id in std::iter::once(key).chain(candidates.iter().map(|(id, _)| format!("native-release-source:{id}"))) {
                sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)")
                    .bind(id).bind(&fingerprint).bind(&value).execute(&mut *tx).await?;
            }
            result
        };
        tx.commit().await?;
        return Ok(json!({"ok":true,"operation":op,"context":{"actor":auth.actor,"chain_id":auth.chain_id},
            "evidence":{"provider_called":false,"physical_cache_hit":"unknown","native_host_owned":true,"replayed":replayed},"result":result}));
    }
    let mut publication_receipt = None;
    let mut replayed = false;
    let requested_operation = op;
    let begin_receipt = if op == "native.begin" {
        if let Some(hash) = p.get("wire_sha256") {
            let hash = hash.as_str().ok_or_else(||CoreError::invalid("wire_sha256 must be SHA256 hex"))?;
            if hash.len()!=64 || !hash.bytes().all(|byte|byte.is_ascii_hexdigit()) {
                return Err(CoreError::invalid("wire_sha256 must be SHA256 hex"));
            }
        }
        p.get("idempotency_key").map(|_| -> Result<_> { Ok((format!("native-begin:{}",storage::digest(&json!([
            auth.actor,auth.chain_id,string(p,"idempotency_key")?]))?),storage::digest(&json!([auth.actor,auth.chain_id,p]))?)) }).transpose()?
    } else { None };
    let mut replay_payload = None;
    if let Some((key,fingerprint)) = &begin_receipt {
        if let Some(row) = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?").bind(key).fetch_optional(&mut *tx).await? {
            let saved: Value = serde_json::from_str(&row.try_get::<String,_>("result")?)?;
            verified.verify(&mut tx,string(&saved,"source_id")?,"native_source").await?;
            if row.try_get::<String,_>("fingerprint")? != *fingerprint {
                return Err(CoreError::conflict("native.begin idempotency key reused with different input or raw wire"));
            }
            reject_released_edit(&mut tx,string(&saved,"source_id")?).await?;
            replay_payload = Some(saved);
            replayed = true;
        }
    }
    let op = if replay_payload.is_some() { "native.get" } else { op };
    let p = replay_payload.as_ref().unwrap_or(p);
    let mut replayed_commit_version = None;
    let result = if op == "native.begin" {
        let format = string(p, "format")?;
        if let Some(step) = p.get("native_step_id") {
            if step.as_str().is_none_or(|step| step.len() != 64 || !step.bytes().all(|byte| byte.is_ascii_hexdigit())) {
                return Err(CoreError::invalid("native_step_id must be SHA256 hex"));
            }
        }
        let request = p
            .get("request")
            .ok_or_else(|| CoreError::invalid("request is required"))?;
        let c = NativeContext::new(format, request).map_err(CoreError::invalid)?;
        let source = storage::uid("native_source");
        let binding = core.register_native_source(&mut tx, auth, format, &c, p).await?;
        if p.get("provenance").is_some() && p.get("provenances").is_some() {
            return Err(CoreError::invalid("use provenance or provenances, not both"));
        }
        let claims=match p.get("provenances") {
            Some(items)=>items.as_array().cloned().ok_or_else(||CoreError::invalid("provenances must be an array"))?,
            None=>p.get("provenance").cloned().into_iter().collect(),
        };
        let mut provenances=Vec::new();
        let mut seen_collections=std::collections::BTreeSet::new();
        let mut occupied_ranges=std::collections::BTreeMap::<String,Vec<(u64,u64)>>::new();
        for claim in claims {
            let proof=validate_collection_provenance(&mut tx,auth,&c,&binding,&claim).await?;
            let identity=(string(&proof,"lifecycle_object_id")?.to_owned(),string(&proof,"collection_receipt_id")?.to_owned());
            if !seen_collections.insert(identity) {return Err(CoreError::forbidden("duplicate collection provenance"));}
            if proof.get("via_volatile_source_id").is_none() {
                for range in proof["text_ranges"].as_array().into_iter().flatten() {
                    let id=string(range,"unit_id")?.to_owned();
                    let begin=version(range,"utf8_start")?;
                    let end=version(range,"utf8_end")?;
                    let occupied=occupied_ranges.entry(id).or_default();
                    if occupied.iter().any(|(a,b)|begin<*b && *a<end) {return Err(CoreError::forbidden("collection material ranges overlap"));}
                    occupied.push((begin,end));
                }
            }
            provenances.push(proof);
        }
        let provenance=if provenances.len()==1 {provenances[0].clone()} else {Value::Null};
        let cb_ids: Vec<Value> = binding["protected_cb_bindings"].as_array().into_iter().flatten().map(|binding|binding["cb_id"].clone()).collect();
        let mut record = json!({"actor":auth.actor,"chain_id":auth.chain_id,
            "core_source_id":binding["core_source_id"],"editable_unit_ids":binding["editable_unit_ids"],
            "protected_cb_bindings":binding["protected_cb_bindings"],"protected_cb_ids":cb_ids,
            "volatile_source_bindings":binding["volatile_source_bindings"],
            "adopted_i":binding["adopted_i"],
            "host_message_origins":binding["host_message_origins"],
            "provenance":provenance,"provenances":provenances,"wire_sha256":p["wire_sha256"],
            "native_step_id":p["native_step_id"],
            "opaque_blocks":binding["opaque_blocks"],"format":format,"request":request,"source_digest":c.source_digest()});
        if let Some(proof) = binding.get("tool_wait") { record["tool_wait"] = proof.clone(); }
        storage::put_object(&mut tx,"native_source",&source,&record).await?;
        storage::put_object(
            &mut tx,
            "native_version",
            &storage::uid("native_version"),
            &json!({"source_id":source,"version":0,"request":request,"receipt":null}),
        )
        .await?;
        view(&source, &c, &Value::Null)?
    } else {
        let id = string(p, "source_id")?;
        let source = verified.read(&mut tx, id, Some("native_source")).await?;
        if source["actor"].as_str() != Some(auth.actor.as_str()) {
            return Err(CoreError::forbidden(
                "native source belongs to another actor",
            ));
        }
        if source.get("chain_id").and_then(Value::as_str) != auth.chain_id.as_deref() {
            return Err(CoreError::forbidden("native source belongs to another chain; unbound candidates require a new native.begin"));
        }
        if let Some(chain) = &auth.chain_id {
            storage::get_chain(&mut tx, chain, &auth.actor).await?;
        }
        core.authorize_native_projection(&mut tx,auth,&source).await?;
        if matches!(op,"native.commit"|"native.publish"|"native.preview") {
            reject_released_edit(&mut tx,id).await?;
        }
        let published = publication(&mut tx,&mut verified,id).await?;
        let current = latest(&mut tx,&mut verified,id).await?;
        let mut c = NativeContext::restore(
            string(&source, "format")?,
            &source["request"],
            &current["request"],
            version(&current, "version")?,
        )
        .map_err(|_| CoreError::new("integrity_error", "persisted native sidecar changed", 500))?;
        if source["source_digest"].as_str() != Some(c.source_digest()) {
            return Err(CoreError::new(
                "integrity_error",
                "native source digest mismatch",
                500,
            ));
        }
        if op == "native.get" {
            publication_receipt = published;
            view(id, &c, &current["receipt"])?
        } else if op=="native.refresh_i" {
            let publication=published.ok_or_else(||CoreError::conflict("I refresh requires an already published candidate"))?;
            let (_,was_replayed)=crate::native_i::refresh(core,&mut tx,&mut verified,auth,&source,&c,p,&publication).await?;
            replayed=was_replayed;publication_receipt=Some(publication);
            view(id,&c,&current["receipt"])?
        } else if op=="native.preview" {
            if published.is_some() {return Err(CoreError::conflict("published candidates are immutable; begin a new scoped candidate"));}
            core.preview_native_candidate(&mut tx,auth,&source,&c,p).await?
        } else if op == "native.publish" {
            let fingerprint = storage::digest(&json!({"actor":auth.actor,"chain_id":auth.chain_id,"payload":p}))?;
            if let Some(previous) = published {
                if previous["fingerprint"] != fingerprint {
                    return Err(CoreError::conflict("native candidate already published with a different transaction"));
                }
                replayed = true;
                publication_receipt = Some(previous);
            } else {
                let mut publish_payload=p.clone();
                if current["delivery"].is_object() {
                    for key in ["ops","pvi"] {
                        if let Some(supplied)=p.get(key) {
                            if *supplied!=current["delivery"][key] {
                                return Err(CoreError::conflict("publication differs from this version's validated delivery"));
                            }
                        }
                        publish_payload[key]=current["delivery"][key].clone();
                    }
                }
                let published = core.publish_native_candidate(&mut tx,auth,&source,&c,&publish_payload).await?;
                let publication_id = storage::uid("native_publication");
                let record = json!({"publication_id":publication_id,"source_id":id,"actor":auth.actor,"chain_id":auth.chain_id,
                    "fingerprint":fingerprint,"version":c.version(),"cb_id":published["cb_id"],
                    "publication":published,"request":c.request().map_err(CoreError::invalid)?});
                storage::put_object(&mut tx,"native_publication",&publication_id,&record).await?;
                publication_receipt = Some(record);
            }
            view(id,&c,&current["receipt"])?
        } else {
            let fingerprint = storage::digest(&json!([auth.actor,auth.chain_id,p]))?;
            let identity = match p.get("idempotency_key") { Some(_) => string(p,"idempotency_key")?,None => &fingerprint };
            let key = format!("native-commit:{}",storage::digest(&json!([auth.actor,auth.chain_id,id,identity]))?);
            let prior = sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?").bind(&key).fetch_optional(&mut *tx).await?;
            if let Some(prior) = prior {
                if prior.try_get::<String,_>("fingerprint")? != fingerprint {
                    return Err(CoreError::conflict("native.commit idempotency key reused with different input"));
                }
                let saved:Value = serde_json::from_str(&prior.try_get::<String,_>("result")?)?;
                replayed_commit_version = saved["committed_version"].as_u64();
                replayed = true;
                publication_receipt = published;
                view(id,&c,&current["receipt"])?
            } else {
            if published.is_some() {
                return Err(CoreError::conflict("published native candidate is immutable; start a new native.begin"));
            }
            if let Some(scope) = source.get("editable_unit_ids") {
                let scope = scope.as_array().ok_or_else(||CoreError::invalid("stored native scope is invalid"))?;
                if p["edits"].as_array().into_iter().flatten().any(|edit|!scope.contains(&edit["unit_id"])) {
                    return Err(CoreError::forbidden("native commit targets a read-only or prior-CB unit"));
                }
            }
            let output = commit(&mut tx, id, &mut c, &source, p).await?;
            sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)").bind(key).bind(fingerprint)
                .bind(storage::json_text(&json!({"source_id":id,"committed_version":c.version()}))?).execute(&mut *tx).await?;
            output
            }
        }
    };
    let mut result = result;
    let source = verified.read(&mut tx,string(&result,"source_id")?,Some("native_source")).await?;
    result["opaque_blocks"] = json!(crate::native_opaque::source_blocks(&source)?);
    result["binding"] = json!({"chain_id":source["chain_id"],"core_source_id":source["core_source_id"],
        "editable_unit_ids":source["editable_unit_ids"],"protected_cb_bindings":source["protected_cb_bindings"],
        "edit_capabilities":{"ops":["KEEP","REPLACE","COMPRESS","DELETE","ADD","REORDER","FLAG"],"pvi":["P","V","I"],"deferred_pvi":[],"required_host_bindings":{"V":"native-volatile-next-WB-v1"},
            "constraints":{"reorder":"pure same-parent contiguous authorized text leaves only","delete":"empty native text leaf; host validates protocol admission","V":"requires trusted next-WB native append and volatile_source_bindings"}},
        "volatile_source_bindings":source["volatile_source_bindings"],
        "adopted_i":source["adopted_i"],
        "host_message_origins":source["host_message_origins"],
        "provenance":source["provenance"],"provenances":source["provenances"],
        "unbound_migration":"unbound candidates stay host-only; begin a new chain-bound candidate to publish"});
    if let Some(proof) = source.get("tool_wait") { result["binding"]["tool_wait"] = proof.clone(); }
    result["publication"] = publication_receipt.unwrap_or(Value::Null);
    let version_record=latest(&mut tx,&mut verified,string(&result,"source_id")?).await?;
    result["delivery"]=version_record["delivery"].clone();
    result["current_delivery"]=result["delivery"].clone();
    result["delivery_digest"]=version_record["delivery_digest"].clone();
    result["delivery_version"]=if result["delivery"].is_null() {Value::Null}else{version_record["version"].clone()};
    if op != "native.preview" {
        if let Some(projection)=crate::native_i::latest(&mut tx,&mut verified,string(&result,"source_id")?,version(&version_record,"version")?).await? {
            core.authorize_native_projection(&mut tx,auth,&projection).await?;
            result["candidate_request"]=result["request"].clone();
            result["candidate_units"]=result["units"].clone();
            result["units"]=json!(NativeContext::new(string(&source,"format")?,&projection["request"]).map_err(CoreError::invalid)?.units().iter().map(|u|json!({"unit_id":u.unit_id,"text":u.text,"anchor":u.anchor})).collect::<Vec<_>>());
            result["request"]=projection["request"].clone();
            result["active_i_projection"]=projection;
        }
    }
    if op == "native.begin" {
        if let Some((key,fingerprint)) = begin_receipt {
            sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)").bind(key).bind(fingerprint)
                .bind(storage::json_text(&json!({"source_id":result["source_id"]}))?).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(
        json!({"ok":true,"operation":requested_operation,"context":{"actor":auth.actor,"chain_id":auth.chain_id},"evidence":{"provider_called":false,"physical_cache_hit":"unknown","native_host_owned":true,"replayed":replayed,"replayed_commit_version":replayed_commit_version},"result":result}),
    )
}

async fn reject_released_edit(conn:&mut SqliteConnection,id:&str)->Result<()> {
    let released:Option<i64> = sqlx::query_scalar("SELECT 1 FROM receipts WHERE key=?")
        .bind(format!("native-release-source:{id}")).fetch_optional(&mut *conn).await?;
    if released.is_some(){return Err(CoreError::conflict("released native candidate is archive-only"));}
    Ok(())
}

async fn publication(conn: &mut SqliteConnection,verified: &mut storage::VerifiedObjects,id: &str) -> Result<Option<Value>> {
    let record: Option<String> = sqlx::query_scalar("SELECT id FROM objects WHERE kind='native_publication' AND json_extract(data,'$.source_id')=? LIMIT 1")
        .bind(id).fetch_optional(&mut *conn).await?;
    match record {
        Some(record) => Ok(Some(verified.read(conn,&record,Some("native_publication")).await?)),
        None => Ok(None),
    }
}

pub(crate) fn collection_proofs(source:&Value)->Vec<&Value> {
    storage::native_collection_proofs(source)
}

/// Validate both raw collection material and an immutable SIC-derived V view.
/// The latter must follow a real V source and its original editing provenance.
pub(crate) async fn validate_collection_provenance(
    conn:&mut SqliteConnection,auth:&OperationContext,context:&NativeContext,binding:&Value,provenance:&Value,
) -> Result<Value> {
    let mut proof=crate::harness_lifecycle::verify_collection(conn,&auth.actor,
        string(provenance,"lifecycle_object_id")?,string(provenance,"collection_receipt_id")?).await?;
    if auth.chain_id.is_none() || proof["chain_id"].as_str()!=auth.chain_id.as_deref() {
        return Err(CoreError::forbidden("collected result provenance belongs to another or unbound chain"));
    }
    let allowed=binding["editable_unit_ids"].as_array().ok_or_else(||CoreError::invalid("native material scope missing"))?;
    let selectors=["unit_ids","native_anchors","native_anchor"].iter().filter(|key|provenance.get(**key).is_some()).count();
    if selectors>1 {return Err(CoreError::invalid("collection material has ambiguous native selectors"));}
    let selected=if let Some(ids)=provenance.get("unit_ids") {ids.clone()}
        else if let Some(anchor)=provenance.get("native_anchor") {
            json!([context.units().iter().find(|unit|unit.anchor==*anchor).map(|unit|unit.unit_id.clone())
                .ok_or_else(||CoreError::forbidden("collection native anchor is not a text leaf"))?])
        } else if let Some(anchors)=provenance.get("native_anchors") {
            let ids:Vec<String>=anchors.as_array().ok_or_else(||CoreError::invalid("native_anchors must be an array"))?.iter()
                .map(|anchor|context.units().iter().find(|unit|unit.anchor==*anchor).map(|unit|unit.unit_id.clone())
                    .ok_or_else(||CoreError::forbidden("collection native anchor is not a text leaf"))).collect::<Result<_>>()?;
            json!(ids)
        } else {json!(allowed)};
    let ids=selected.as_array().ok_or_else(||CoreError::invalid("provenance unit_ids must be an array"))?;
    if ids.is_empty() || ids.iter().any(|id|!allowed.contains(id)) {
        return Err(CoreError::forbidden("collection provenance must target new authorized native material"));
    }
    let canonical:Vec<Value>=context.units().iter().map(|unit|json!(unit.unit_id)).filter(|id|ids.contains(id)).collect();
    if *ids!=canonical {
        return Err(CoreError::forbidden("collection provenance units must be unique and follow native source order"));
    }
    if let Some(via)=provenance.get("via_volatile_source_id") {
        let via=via.as_str().ok_or_else(||CoreError::invalid("volatile provenance source must be text"))?;
        let source=storage::get_object(conn,via,Some("source")).await?;
        let inherited=source["collection_provenances"].as_array().into_iter().flatten().find(|item|
            item["lifecycle_object_id"]==proof["lifecycle_object_id"] && item["collection_receipt_id"]==proof["collection_receipt_id"])
            .or_else(||source.get("collection_provenance").filter(|item|item.is_object()))
            .ok_or_else(||CoreError::forbidden("volatile source has no collection derivation"))?;
        for key in ["lifecycle_object_id","collection_receipt_id","collection_digest"] {
            if inherited[key]!=proof[key] {
                return Err(CoreError::forbidden("volatile source does not derive from this collection"));
            }
        }
        let origin=storage::get_object(conn,string(inherited,"origin_native_source_id")?,Some("native_source")).await?;
        if origin["actor"]!=auth.actor || origin["chain_id"].as_str()!=auth.chain_id.as_deref() {
            return Err(CoreError::forbidden("volatile provenance origin belongs to another actor or chain"));
        }
        let original=NativeContext::new(string(&origin,"format")?,&origin["request"]).map_err(CoreError::invalid)?;
        let original_proof=origin["provenances"].as_array().into_iter().flatten().find(|item|
            item["lifecycle_object_id"]==proof["lifecycle_object_id"] && item["collection_receipt_id"]==proof["collection_receipt_id"])
            .or_else(||origin.get("provenance").filter(|item|item.is_object()))
            .ok_or_else(||CoreError::forbidden("volatile collection origin proof missing"))?;
        Box::pin(validate_collection_provenance(conn,auth,&original,&origin,original_proof)).await?;
        let native=binding["volatile_source_bindings"].as_array().into_iter().flatten()
            .find(|binding|binding["source_id"]==via).ok_or_else(||CoreError::forbidden("volatile result was not appended by this native source"))?;
        let native_ids=native["unit_ids"].as_array().ok_or_else(||CoreError::invalid("volatile native unit mapping missing"))?;
        let old_units=source["units"].as_array().ok_or_else(||CoreError::invalid("volatile source units missing"))?;
        let inherited_ids=inherited["unit_ids"].as_array().ok_or_else(||CoreError::invalid("volatile collection unit provenance missing"))?;
        for id in ids {
            let index=native_ids.iter().position(|native|native==id).ok_or_else(||CoreError::forbidden("collection material is outside its volatile append"))?;
            let old=old_units.get(index).ok_or_else(||CoreError::invalid("volatile material index missing"))?;
            if !inherited_ids.contains(&old["unit_id"]) {
                return Err(CoreError::forbidden("this volatile unit has no collection derivation"));
            }
            let unit=context.units().iter().find(|unit|Some(unit.unit_id.as_str())==id.as_str()).expect("validated native ID");
            if old["text"].as_str()!=Some(unit.text.as_str()) {
                return Err(CoreError::conflict("appended collection view differs from its immutable volatile source"));
            }
        }
        proof["via_volatile_source_id"]=json!(via);
    } else {
        let explicit=provenance.get("text_ranges").cloned();
        let shorthand=provenance.get("utf8_start").is_some() || provenance.get("utf8_end").is_some();
        if shorthand && (ids.len()!=1 || explicit.is_some()) {return Err(CoreError::invalid("ambiguous collection byte range"));}
        let ranges=if let Some(ranges)=explicit {
            ranges.as_array().cloned().ok_or_else(||CoreError::invalid("text_ranges must be an array"))?
        } else if shorthand {
            vec![json!({"unit_id":ids[0],"utf8_start":version(provenance,"utf8_start")?,"utf8_end":version(provenance,"utf8_end")?})]
        } else {
            ids.iter().map(|id|{
                let unit=context.units().iter().find(|unit|Some(unit.unit_id.as_str())==id.as_str()).expect("validated material unit");
                json!({"unit_id":id,"utf8_start":0,"utf8_end":unit.text.len()})
            }).collect()
        };
        if ranges.len()!=ids.len() {return Err(CoreError::forbidden("one byte range is required per collection native unit"));}
        let mut material=Vec::new();
        let mut normalized_ranges=Vec::new();
        for (id,range) in ids.iter().zip(&ranges) {
            let range_id=if let Some(id)=range.get("unit_id") {id.clone()} else {
                json!(context.units().iter().find(|unit|unit.anchor==range["native_anchor"]).map(|unit|unit.unit_id.clone())
                    .ok_or_else(||CoreError::forbidden("collection range anchor missing"))?)
            };
            if range_id!=*id {return Err(CoreError::forbidden("collection ranges must be unique and follow native source order"));}
            let unit=context.units().iter().find(|unit|Some(unit.unit_id.as_str())==id.as_str()).expect("validated unit");
            let begin=usize::try_from(version(range,"utf8_start")?).map_err(|_|CoreError::invalid("collection range overflow"))?;
            let end=usize::try_from(version(range,"utf8_end")?).map_err(|_|CoreError::invalid("collection range overflow"))?;
            if begin>=end {return Err(CoreError::forbidden("collection material range must be nonempty"));}
            normalized_ranges.push(json!({"unit_id":id,"utf8_start":begin,"utf8_end":end}));
            let slice=unit.text.get(begin..end).ok_or_else(||CoreError::forbidden("collection range must stay within UTF8 text boundaries"))?;
            material.push(slice.to_owned());
        }
        proof["text_ranges"]=json!(normalized_ranges);
        if storage::sha256(material.join("\n").as_bytes())!=proof["collection_digest"] {
            return Err(CoreError::conflict("native source does not contain the collected result text"));
        }
    }
    proof["unit_ids"]=json!(ids);
    Ok(proof)
}
