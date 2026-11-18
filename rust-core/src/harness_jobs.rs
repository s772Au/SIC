//! Harness execution handles; SIC lifecycle, cursors and cancellation intents.
use super::*;
static NOTIFY: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
pub(super) fn notify() {
    NOTIFY
        .get_or_init(tokio::sync::Notify::new)
        .notify_waiters();
}
pub(super) async fn wait(store: &Store, a: &OperationContext, p: &Value) -> Result<()> {
    let mut tx = store.begin().await?;
    if replay(&mut tx, a, p, "harness.call").await?.is_some() {
        return Ok(());
    }
    let s = resolve(&mut tx, a, p).await?;
    let g = authorize(&mut tx, a, p, "job_output", s["object_id"].as_str(), false).await?;
    if let Some(launch) = &g.launch {
        let key = format!(
            "harness_native_call_{}",
            storage::digest(&json!([
                a.actor,
                text(p, "scope_id")?,
                g.chain_id,
                launch.call_id,
                "job_output"
            ]))?
        );
        match storage::get_object(&mut tx, &key, Some("harness_native_call")).await {
            Ok(_) => return Ok(()),
            Err(error) if error.code == "not_found" => (),
            Err(error) => return Err(error),
        }
    }
    // The Harness can pass its already-clamped existing wait setting. The
    // fallback is the native DSH job_output default, not a new SIC model cap.
    let duration = g
        .wait_timeout_ms
        .or_else(|| p["arguments"]["timeout_ms"].as_u64())
        .unwrap_or(30_000);
    tx.commit().await?;
    let end = tokio::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(duration))
        .ok_or_else(|| CoreError::invalid("wait duration is outside the clock range"))?;
    loop {
        let notified = NOTIFY.get_or_init(tokio::sync::Notify::new).notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let mut tx = store.begin().await?;
        let current = resolve(&mut tx, a, p).await?;
        tx.commit().await?;
        if terminal(&current) {
            return Ok(());
        }
        if tokio::time::timeout_at(end, notified).await.is_err() {
            return Ok(());
        }
    }
}
fn object_id(actor: &str, scope: &str, _kind: &str, external: &str) -> Result<String> {
    Ok(format!(
        "harness_activity_{}",
        storage::digest(&json!([actor, scope, external]))?
    ))
}
pub(super) async fn observe(
    conn: &mut sqlx::Transaction<'_,sqlx::Sqlite>,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    if let Some(done) = replay(conn, a, p, "harness.observe").await? {
        return Ok(done);
    }
    let id = text(p, "object_id")?;
    let mut s = owned(conn, &a.actor, text(p, "scope_id")?, id).await?;
    if integer(p, "expected_revision")? != integer(&s, "revision")? {
        return Err(CoreError::conflict("activity revision conflict"));
    }
    let o = &p["observation"];
    let name = text(o, "kind")?;
    authorize(conn, a, p, name, Some(id), true).await?;
    if !matches!(s["object_kind"].as_str(), Some("job" | "workflow" | "peer")) {
        return Err(CoreError::invalid("observation is not for an activity"));
    }
    match name {
        "job.output" => {
            if terminal(&s) {
                return Err(CoreError::conflict("settled job cannot append output"));
            }
            let text = o["text"]
                .as_str()
                .ok_or_else(|| CoreError::invalid("text must be a string"))?;
            s["chunks"]
                .as_array_mut()
                .ok_or_else(|| CoreError::invalid("activity output log invalid"))?
                .push(json!(text));
            if s["chunk_metadata"].is_null() {
                s["chunk_metadata"] =
                    json!(vec![Value::Null; s["chunks"].as_array().unwrap().len() - 1]);
            }
            if let Some(chunk) = o.get("native_chunk") {
                if chunk["text"] != text || !chunk["at"].is_u64() {
                    return Err(CoreError::invalid(
                        "native output chunk does not match text or offset",
                    ));
                }
                if chunk
                    .get("channel")
                    .is_some_and(|v| !matches!(v.as_str(), Some("stdout" | "stderr" | "log")))
                {
                    return Err(CoreError::invalid("invalid native output channel"));
                }
            }
            s["chunk_metadata"]
                .as_array_mut()
                .unwrap()
                .push(o["native_chunk"].clone());
        }
        "job.native_view" => {
            if !o["view"].is_object() || o["view"]["id"] != s["external_id"] {
                return Err(CoreError::invalid(
                    "native view must describe the bound executor",
                ));
            }
            s["native_view"] = o["view"].clone();
            for (native, core) in [("label", "label"), ("kind", "job_kind")] {
                if let Some(value) = o["view"][native].as_str().filter(|value| !value.is_empty()) {
                    s[core] = json!(value);
                }
            }
            if let Some(started) = o["view"]["startedAt"].as_u64() {
                s["startedAt"] = json!(started);
            }
        }
        "job.tool_result" => {
            if !o["result"].is_object() || !o["result"]["isError"].is_boolean() {
                return Err(CoreError::invalid("original native tool result required"));
            }
            if !s["native_tool_result"].is_null() && s["native_tool_result"] != o["result"] {
                return Err(CoreError::conflict("native tool result already committed"));
            }
            s["native_tool_result"] = o["result"].clone();
        }
        "job.stopping" => {
            if !terminal(&s) {
                s["status"] = json!("stopping");
            }
        }
        "job.settle" => {
            if terminal(&s) {
                return Err(CoreError::conflict("job already settled"));
            }
            let status = text(o, "status")?;
            if !matches!(status, "completed" | "killed" | "failed") {
                return Err(CoreError::invalid(
                    "settlement must be completed, killed or failed",
                ));
            }
            s["status"] = json!(status);
            s["finishedAt"] = json!(integer(o, "finished_at_ms")?);
            if let Some(result) = o.get("result") {
                if !result.is_string() {
                    return Err(CoreError::invalid("job result must be text"));
                }
                s["result"] = result.clone();
            }
            if let Some(detail) = o.get("detail") {
                if !detail.is_string() {
                    return Err(CoreError::invalid("detail must be text"));
                }
                s["detail"] = detail.clone();
            }
        }
        "result.adopt" => {
            let probe=adoption_probe(conn,a,id,o,true).await?;
            if probe["eligible_current_p"]!=true {return Err(CoreError::forbidden("collection has no surviving P representation in the adopted cut"));}
            let collection=text(o,"collection_receipt_id")?;let publication_id=text(o,"publication_id")?;
            s["adopted"] = json!(true);
            if s["adoptions"].is_null() {
                s["adoptions"] = json!([]);
            }
            s["adoptions"].as_array_mut().unwrap().push(json!({"collection_receipt_id":collection,"publication_id":publication_id,"cb_id":probe["cb_id"],"adopted_cut_id":probe["adopted_cut_id"],"p_lineage":probe["p_lineage"]}));
        }
        _ => return Err(CoreError::invalid("unsupported activity observation")),
    }
    if terminal(&s) {
        launch::settled(conn, &mut s).await?;
    }
    s["revision"] = json!(integer(&s, "revision")? + 1);
    save(conn, &s).await?;
    receipt(conn, a, p, "harness.observe", &s, json!({"job":public(&s)})).await
}
fn public(s: &Value) -> Value {
    let mut v = json!({"id":s["external_id"],"kind":s["job_kind"],"label":s["label"],"status":s["status"],"startedAt":s["startedAt"]});
    for key in ["detail", "finishedAt"] {
        if !s[key].is_null() {
            v[key] = s[key].clone();
        }
    }
    if matches!(s["status"].as_str(), Some("running" | "stopping")) {
        if let Some(progress) = s["native_view"]["progress"].as_str() {
            v["detail"] = json!(progress);
        }
    }
    if s["status"] == "unknown" {
        v["status"] = json!("running");
        v["detail"] =
            json!("executor state unknown; reconciliation required; no redispatch permit");
    }
    v
}
fn terminal(s: &Value) -> bool {
    matches!(
        s["status"].as_str(),
        Some("completed" | "killed" | "failed")
    )
}
async fn resolve(conn: &mut SqliteConnection, a: &OperationContext, p: &Value) -> Result<Value> {
    let external = text(&p["arguments"], "job_id")?;
    let bid = launch::binding_id(&a.actor, text(p, "scope_id")?, external)?;
    match storage::get_object(conn, &bid, Some("harness_executor_binding")).await {
        Ok(binding) => {
            return owned(
                conn,
                &a.actor,
                text(p, "scope_id")?,
                text(&binding, "object_id")?,
            )
            .await;
        }
        Err(e) if e.code == "not_found" => (),
        Err(e) => return Err(e),
    }
    let id = object_id(
        &a.actor,
        text(p, "scope_id")?,
        "job",
        text(&p["arguments"], "job_id")?,
    )?;
    owned(conn, &a.actor, text(p, "scope_id")?, &id).await
}
pub(super) async fn call(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    let name = text(p, "name")?;
    let scope = text(p, "scope_id")?;
    if name == "job_list" {
        let mut list = Vec::new();
        let mut bindings = Vec::new();
        for kind in ["job", "workflow", "peer"] {
            for id in ids(conn, &a.actor, scope, kind).await? {
                let s = owned(conn, &a.actor, scope, &id).await?;
                if s["visible"] == false || s["external_id"].is_null() {
                    continue;
                }
                list.push(public(&s));
                bindings.push(json!({"id":s["external_id"],"object_id":id,"revision":s["revision"],"object_kind":kind,"executor":s["executor"]}));
            }
        }
        return Ok(json!({"value":list,"bindings":bindings}));
    }
    if let Some(done) = replay(conn, a, p, "harness.call").await? {
        return Ok(done);
    }
    let mut s = resolve(conn, a, p).await?;
    let id = s["object_id"].as_str().unwrap().to_owned();
    if p.get("expected_revision").is_some()
        && integer(p, "expected_revision")? != integer(&s, "revision")?
    {
        return Err(CoreError::conflict("job revision conflict"));
    }
    let grant = authorize(conn, a, p, name, Some(&id), false).await?;
    let native_call = if let Some(launch) = grant.launch.as_ref() {
        Some((
            format!(
                "harness_native_call_{}",
                storage::digest(&json!([
                    a.actor,
                    scope,
                    grant.chain_id,
                    launch.call_id,
                    name
                ]))?
            ),
            json!({"object_id":id,"native_name":launch.native_name,"arguments_sha256":launch.arguments_sha256,"arguments":p["arguments"]}),
        ))
    } else {
        None
    };
    if let Some((key, fingerprint)) = &native_call {
        if grant.launch.as_ref().unwrap().native_name != name {
            return Err(CoreError::forbidden(
                "native control name differs from trusted call identity",
            ));
        }
        match storage::get_object(conn, key, Some("harness_native_call")).await {
            Ok(previous) => {
                if previous["fingerprint"] != *fingerprint {
                    return Err(CoreError::conflict(
                        "native control identity reused with different invocation",
                    ));
                }
                let mut result = previous["result"].clone();
                result["replayed"] = json!(true);
                return Ok(result);
            }
            Err(error) if error.code == "not_found" => (),
            Err(error) => return Err(error),
        }
    }
    let value = match name {
        "job_output" => collect(&mut s, a, p)?,
        "job_kill" => {
            let outcome = if terminal(&s) {
                "already-finished"
            } else {
                if s["status"] != "stopping" {
                    s["status"] = json!("stopping");
                    s["cancel_request_id"] = json!(event_key(a, p)?);
                    s["cancel_reason"] = p["arguments"]["reason"].clone();
                }
                "cancellation-requested"
            };
            json!({"outcome":outcome,"job":public(&s)})
        }
        _ => return Err(CoreError::invalid("unsupported job tool")),
    };
    s["revision"] = json!(integer(&s, "revision")? + 1);
    save(conn, &s).await?;
    let result = receipt(conn, a, p, "harness.call", &s, value).await?;
    if let Some((key, fingerprint)) = native_call {
        storage::put_object(
            conn,
            "harness_native_call",
            &key,
            &json!({"fingerprint":fingerprint,"result":result}),
        )
        .await?;
    }
    Ok(result)
}
fn collect(s: &mut Value, a: &OperationContext, p: &Value) -> Result<Value> {
    let chunks = s["chunks"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("invalid job output log"))?;
    let cursor = integer(s, "output_cursor")? as usize;
    if cursor > chunks.len() {
        return Err(CoreError::invalid("invalid output cursor"));
    }
    let mut output = String::new();
    let mut stderr = String::new();
    let mut lossy = false;
    let earliest = s["native_view"]["output"]["earliest"].as_u64().unwrap_or(0);
    if s["output_mode"] == "stream" || terminal(s) {
        for (index, chunk) in chunks[cursor..].iter().enumerate() {
            let mut text = chunk
                .as_str()
                .ok_or_else(|| CoreError::invalid("invalid output chunk"))?;
            let metadata = &s["chunk_metadata"][cursor + index];
            if let Some(at) = metadata["at"].as_u64() {
                if at.saturating_add(text.len() as u64) <= earliest {
                    lossy = true;
                    continue;
                }
                if at < earliest {
                    let mut cut = (earliest - at) as usize;
                    while cut < text.len() && !text.is_char_boundary(cut) {
                        cut += 1;
                    }
                    text = &text[cut..];
                    lossy = true;
                }
            }
            if metadata["channel"] != "log" && metadata["gapBefore"] == true {
                lossy = true;
            }
            match metadata["channel"].as_str() {
                Some("log") => (),
                Some("stderr") => stderr.push_str(text),
                _ => output.push_str(text),
            }
        }
        if !stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("[stderr]\n");
            output.push_str(&stderr);
        }
        s["output_cursor"] = json!(chunks.len());
        s["native_model_cursor"] = s["native_view"]["output"]["total"].clone();
    }
    if lossy {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        let paths = s["native_view"]["output"]["spillPaths"]
            .as_array()
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        output.push_str(&format!(
            "[some output was dropped from memory; full output: {}]",
            if paths.is_empty() {
                "(unavailable)"
            } else {
                &paths
            }
        ));
    }
    if terminal(s) && s["result_collected"] != true {
        if let Some(result) = s["result"].as_str() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(result);
        }
        s["result_collected"] = json!(true);
    }
    s["last_collection"] =
        json!({"receipt_id":event_key(a,p)?,"text_digest":storage::sha256(output.as_bytes())});
    Ok(json!({"text":output,"job":public(s)}))
}
pub(super) async fn register(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    if let Some(done) = replay(conn, a, p, "harness.observe").await? {
        return Ok(done);
    }
    let o = &p["observation"];
    let name = text(o, "kind")?;
    let kind = match name {
        "job.register" => "job",
        "workflow.register" => "workflow",
        "peer.register" => "peer",
        _ => return Err(CoreError::invalid("unsupported lifecycle registration")),
    };
    let grant = authorize(conn, a, p, name, None, false).await?;
    let external = text(o, "external_id")?;
    let scope = text(p, "scope_id")?;
    let binding = launch::binding_id(&a.actor, scope, external)?;
    match storage::get_object(conn, &binding, Some("harness_executor_binding")).await {
        Ok(binding) => {
            let current = owned(conn, &a.actor, scope, text(&binding, "object_id")?).await?;
            if current["executor"] != o["executor"] {
                return Err(CoreError::conflict(
                    "native handle executor differs from its authoritative binding",
                ));
            }
            return receipt(
                conn,
                a,
                p,
                "harness.observe",
                &current,
                json!({"job":public(&current)}),
            )
            .await;
        }
        Err(error) if error.code == "not_found" => (),
        Err(error) => return Err(error),
    }
    let id = object_id(&a.actor, scope, kind, external)?;
    if sqlx::query_scalar::<_, i64>("SELECT 1 FROM objects WHERE id=?")
        .bind(&id)
        .fetch_optional(&mut *conn)
        .await?
        .is_some()
    {
        return Err(CoreError::conflict("lifecycle identity already registered"));
    }
    if !o["executor"].is_object() {
        return Err(CoreError::invalid("executor mapping must be an object"));
    }
    text(&o["executor"], "kind")?;
    text(&o["executor"], "id")?;
    let mode = o["output_mode"].as_str().unwrap_or("stream");
    if !matches!(mode, "stream" | "final") {
        return Err(CoreError::invalid("output_mode must be stream or final"));
    }
    let started = integer(o, "started_at_ms")?;
    storage::put_object(
        conn,
        "harness_object",
        &id,
        &json!({"actor":a.actor,"scope_id":scope,"object_kind":kind,"external_id":external,"chain_id":grant.chain_id}),
    )
    .await?;
    let s = json!({"object_id":id,"object_kind":kind,"revision":1,"external_id":external,"job_kind":o["job_kind"].as_str().unwrap_or(kind),"label":text(o,"label")?,"status":"running","startedAt":started,"executor":o["executor"],"output_mode":mode,"chunks":[],"output_cursor":0,"result_collected":false,"adopted":false});
    save(conn, &s).await?;
    receipt(conn, a, p, "harness.observe", &s, json!({"job":public(&s)})).await
}

/// Read-only publication/receipt authorization plus actual selected P lineage.
pub(super) async fn adoption_probe(conn:&mut sqlx::Transaction<'_,sqlx::Sqlite>,a:&OperationContext,id:&str,o:&Value,require_p:bool)->Result<Value>{
    let collection = text(o, "collection_receipt_id")?;
    let proof = verify_collection(conn, &a.actor, id, collection).await?;
    let publication_id = text(o, "publication_id")?;
    let publication =
        storage::get_object(conn, publication_id, Some("native_publication")).await?;
    let source = storage::get_object(
        conn,
        text(&publication, "source_id")?,
        Some("native_source"),
    )
    .await?;
    let provenance_matches=crate::native_transactions::collection_proofs(&source).iter().any(|proof|
        proof["lifecycle_object_id"]==id && proof["collection_receipt_id"]==collection);
    let eligibility=publication["publication"]["collection_adoptions"].as_array().into_iter().flatten().find(|item|
        item["lifecycle_object_id"]==id && item["collection_receipt_id"]==collection)
        .unwrap_or(&publication["publication"]["collection_adoption"]);
    if publication["actor"] != a.actor
        || proof["chain_id"].is_null()
        || a.chain_id.as_deref().is_some_and(|chain|proof["chain_id"].as_str()!=Some(chain))
        || publication["chain_id"] != proof["chain_id"]
        || source["chain_id"] != proof["chain_id"]
        || !provenance_matches
        || eligibility["lifecycle_object_id"] != id
        || eligibility["collection_receipt_id"] != collection
        || eligibility["collection_digest"] != proof["collection_digest"]
    {
        return Err(CoreError::forbidden(
            "publication does not adopt this collected result in its chain",
        ));
    }
    storage::get_object(conn, text(&publication, "cb_id")?, Some("cb")).await?;
    let wanted=match o.get("adopted_cut_id") {Some(value)=>Some(value.as_str().filter(|id|!id.is_empty()).ok_or_else(||CoreError::invalid("adopted_cut_id must be nonempty text"))?),None=>None};
    let mut lineage=if require_p {crate::core::verify_collection_p_lineage(conn,&a.actor,text(&publication,"chain_id")?,text(&publication,"cb_id")?,wanted).await?}else{crate::core::query_collection_p_lineage(conn,&a.actor,text(&publication,"chain_id")?,text(&publication,"cb_id")?,wanted).await?};
    lineage["publication_eligible_current_p"]=eligibility["eligible_current_p"].clone();
    lineage["eligible_current_p"]=json!(eligibility["eligible_current_p"]==true && lineage["eligible_current_p"]==true);
    Ok(json!({"publication_id":publication_id,"collection_receipt_id":collection,"lifecycle_object_id":id,"cb_id":publication["cb_id"],"eligible_current_p":lineage["eligible_current_p"],"adopted_cut_id":lineage["adopted_cut_id"],"p_lineage":lineage}))
}
