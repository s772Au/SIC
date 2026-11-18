//! One-shot dispatch using the existing host_tool_tasks claim ledger. This is
//! a lease/intent protocol, never an executor. Replays never return a permit.
use super::*;
pub(super) async fn observe(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    if let Some(done) = replay(conn, a, p, "harness.observe").await? {
        return Ok(done);
    }
    let o = &p["observation"];
    let name = text(o, "kind")?;
    if name.ends_with(".intent") {
        return intent(conn, a, p).await;
    }
    let id = text(p, "object_id")?;
    let mut s = owned(conn, &a.actor, text(p, "scope_id")?, id).await?;
    if integer(p, "expected_revision")? != integer(&s, "revision")? {
        return Err(CoreError::conflict("launch lifecycle revision conflict"));
    }
    if !matches!(s["object_kind"].as_str(), Some("job" | "workflow" | "peer")) {
        return Err(CoreError::invalid("launch is not an activity"));
    }
    let g = authorize(conn, a, p, name, Some(id), name != "job.claim").await?;
    let mut t = if s["host_task_id"].is_string() {
        task(conn, &s).await?
    } else {
        json!({"status":s["status"]})
    };
    let mut permit = false;
    match name {
        "job.claim" => {
            if !s["host_task_id"].is_string() {
                return Err(CoreError::conflict(
                    "observation-only activity has no launch permit",
                ));
            }
            if t["status"] == "pending" {
                t["status"] = json!("claimed");
                t["claim_id"] = json!(text(o, "claim_id")?);
                t["claimed_by"] = json!(a.actor);
                t["claim_turn_id"] = json!(g.turn_id);
                s["dispatch_state"] = json!("claimed");
                permit = true;
            }
        }
        "job.attach" => attach(conn, a, p, &mut s, &mut t).await?,
        "job.recover" => {
            if matches!(
                t["status"].as_str(),
                Some("claimed" | "attached" | "unknown")
            ) || !s["host_task_id"].is_string()
            {
                t["status"] = json!("unknown");
                s["dispatch_state"] = json!("unknown");
                s["status"] = json!("unknown");
                s["detail"] = json!("executor outcome unknown; reconcile without redispatch");
            }
        }
        "job.reconcile" => {
            if !matches!(
                t["status"].as_str(),
                Some("unknown" | "attached" | "claimed")
            ) {
                return Err(CoreError::conflict(
                    "activity is not awaiting reconciliation",
                ));
            }
            match text(o, "outcome")? {
                "running" => {
                    if s["external_id"].is_null() {
                        return Err(CoreError::conflict(
                            "attach an observed executor handle first",
                        ));
                    }
                    t["status"] = json!("attached");
                    s["dispatch_state"] = json!("attached");
                    s["status"] = json!("running");
                    s["detail"] = Value::Null;
                }
                "not_started" => {
                    t["status"] = json!("not_started");
                    s["dispatch_state"] = json!("not_started");
                    s["status"] = json!("failed");
                    s["detail"] = json!(
                        "executor attested that launch did not start; no automatic retry permit"
                    );
                }
                "unknown" => {
                    t["status"] = json!("unknown");
                    s["dispatch_state"] = json!("unknown");
                    s["status"] = json!("unknown");
                }
                _ => {
                    return Err(CoreError::invalid(
                        "reconcile outcome must be running, not_started or unknown",
                    ));
                }
            }
        }
        "job.remove" => {
            s["visible"] = json!(false);
            s["removal_reason"] = json!(text(o, "reason")?);
        }
        _ => return Err(CoreError::invalid("unsupported launch control")),
    }
    if s["host_task_id"].is_string() {
        update_task(conn, &t).await?;
    }
    s["revision"] = json!(integer(&s, "revision")? + 1);
    save(conn, &s).await?;
    record(conn, a, p, &s, &t, permit).await
}
pub(super) fn binding_id(actor: &str, scope: &str, external: &str) -> Result<String> {
    Ok(format!(
        "harness_binding_{}",
        storage::digest(&json!([actor, scope, external]))?
    ))
}
async fn task(conn: &mut SqliteConnection, s: &Value) -> Result<Value> {
    let encoded: Option<String> =
        sqlx::query_scalar("SELECT state FROM host_tool_tasks WHERE id=?")
            .bind(text(s, "host_task_id")?)
            .fetch_optional(conn)
            .await?;
    let value: Value = serde_json::from_str(
        &encoded.ok_or_else(|| CoreError::new("not_found", "launch task missing", 404))?,
    )?;
    if value["domain"] != "harness_launch" || value["object_id"] != s["object_id"] {
        return Err(CoreError::conflict("launch task domain mismatch"));
    }
    Ok(value)
}
async fn update_task(conn: &mut SqliteConnection, t: &Value) -> Result<()> {
    let changed = sqlx::query("UPDATE host_tool_tasks SET state=? WHERE id=?")
        .bind(storage::json_text(t)?)
        .bind(text(t, "task_id")?)
        .execute(conn)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(CoreError::conflict("launch task missing"));
    }
    Ok(())
}
async fn attach(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    s: &mut Value,
    t: &mut Value,
) -> Result<()> {
    let o = &p["observation"];
    let claim = text(o, "claim_id")?;
    if !matches!(
        t["status"].as_str(),
        Some("claimed" | "unknown" | "attached")
    ) || t["claim_id"] != claim
    {
        return Err(CoreError::conflict(
            "executor handle does not match the one-shot launch claim",
        ));
    }
    let external = text(o, "external_id")?;
    if !s["external_id"].is_null() && s["external_id"] != external {
        return Err(CoreError::conflict(
            "native executor id is already attached",
        ));
    }
    text(&o["executor"], "kind")?;
    text(&o["executor"], "id")?;
    let bid = binding_id(&a.actor, text(p, "scope_id")?, external)?;
    let legacy_id = format!(
        "harness_activity_{}",
        storage::digest(&json!([a.actor, text(p, "scope_id")?, external]))?
    );
    if legacy_id != s["object_id"].as_str().unwrap_or("")
        && sqlx::query_scalar::<_, i64>("SELECT 1 FROM objects WHERE id=?")
            .bind(&legacy_id)
            .fetch_optional(&mut *conn)
            .await?
            .is_some()
    {
        return Err(CoreError::conflict(
            "native handle is already registered under another lifecycle object",
        ));
    }
    let binding = json!({"actor":a.actor,"scope_id":p["scope_id"],"object_id":s["object_id"],"external_id":external,"executor":o["executor"]});
    match storage::get_object(conn, &bid, Some("harness_executor_binding")).await {
        Ok(previous) => {
            if previous != binding {
                return Err(CoreError::conflict(
                    "native handle belongs to another logical launch",
                ));
            }
        }
        Err(e) if e.code == "not_found" => {
            storage::put_object(conn, "harness_executor_binding", &bid, &binding).await?
        }
        Err(e) => return Err(e),
    }
    s["external_id"] = json!(external);
    s["executor"] = o["executor"].clone();
    s["status"] = json!("running");
    s["dispatch_state"] = json!("attached");
    s["detail"] = Value::Null;
    if let Some(started) = o["started_at_ms"].as_u64() {
        s["startedAt"] = json!(started);
    }
    t["status"] = json!("attached");
    t["external_id"] = json!(external);
    t["executor"] = o["executor"].clone();
    Ok(())
}
pub(super) async fn settled(conn: &mut SqliteConnection, s: &mut Value) -> Result<()> {
    if s["host_task_id"].is_string() {
        let mut t = task(conn, s).await?;
        t["status"] = json!("completed");
        t["settled_status"] = s["status"].clone();
        update_task(conn, &t).await?;
        s["dispatch_state"] = json!("settled");
    }
    Ok(())
}
async fn record(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    s: &Value,
    t: &Value,
    permit: bool,
) -> Result<Value> {
    receipt_extra(conn,a,p,"harness.observe",s,json!({"activity":s}),json!({"task_id":t["task_id"],"lease_id":t["lease_id"],"execution_permit":permit,"dispatch_state":t["status"]})).await
}
async fn intent(conn: &mut SqliteConnection, a: &OperationContext, p: &Value) -> Result<Value> {
    let o = &p["observation"];
    let name = text(o, "kind")?;
    let g = authorize(conn, a, p, name, None, false).await?;
    let identity = g
        .launch
        .as_ref()
        .ok_or_else(|| CoreError::forbidden("trusted native launch identity required"))?;
    if identity.call_id.trim().is_empty()
        || identity.native_name.trim().is_empty()
        || identity.arguments_sha256.len() != 64
        || !identity
            .arguments_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(CoreError::invalid("invalid trusted launch identity"));
    }
    let scope = text(p, "scope_id")?;
    let key = storage::digest(&json!([a.actor, scope, g.chain_id, identity.call_id]))?;
    let id = format!("harness_activity_{key}");
    let task_id = format!("harness_launch_{key}");
    let existing: Option<String> =
        sqlx::query_scalar("SELECT state FROM host_tool_tasks WHERE id=?")
            .bind(&task_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(encoded) = existing {
        let t: Value = serde_json::from_str(&encoded)?;
        if t["intent_kind"] != name
            || t["native_tool_name"] != identity.native_name
            || t["arguments_sha256"] != identity.arguments_sha256
        {
            return Err(CoreError::conflict(
                "logical launch identity reused with different invocation",
            ));
        }
        let s = owned(conn, &a.actor, scope, &id).await?;
        return record(conn, a, p, &s, &t, false).await;
    }
    let kind = match name {
        "job.intent" => "job",
        "workflow.intent" => "workflow",
        "peer.intent" => "peer",
        _ => return Err(CoreError::invalid("invalid activity intent")),
    };
    let lease = storage::uid("harness_lease");
    let t = json!({"domain":"harness_launch","intent_kind":name,"task_id":task_id,"object_id":id,"actor":a.actor,"scope_id":scope,"chain_id":g.chain_id,"status":"pending","logical_call_id":identity.call_id,"native_tool_name":identity.native_name,"arguments_sha256":identity.arguments_sha256,"lease_id":lease});
    sqlx::query("INSERT INTO host_tool_tasks(id,state) VALUES(?,?)")
        .bind(&task_id)
        .bind(storage::json_text(&t)?)
        .execute(&mut *conn)
        .await?;
    storage::put_object(conn,"harness_object",&id,&json!({"actor":a.actor,"scope_id":scope,"object_kind":kind,"chain_id":g.chain_id,"launch_identity":identity})).await?;
    let mode = o["output_mode"].as_str().unwrap_or("stream");
    if !matches!(mode, "stream" | "final") {
        return Err(CoreError::invalid("invalid output mode"));
    }
    let s = json!({"object_id":id,"object_kind":kind,"revision":1,"external_id":null,"job_kind":o["job_kind"].as_str().unwrap_or(kind),"label":text(o,"label")?,"status":"pending","startedAt":o["started_at_ms"].as_u64().unwrap_or(0),"output_mode":mode,"chunks":[],"output_cursor":0,"result_collected":false,"adopted":false,"visible":true,"dispatch_state":"pending","host_task_id":task_id});
    save(conn, &s).await?;
    record(conn, a, p, &s, &t, false).await
}
