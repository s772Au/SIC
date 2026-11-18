//! SIC-owned persistent lifecycle; Harness grants carry trusted human/round
//! provenance. Tool arguments never grant authority. No scheduler or executor
//! is implemented here: hosts dispatch work and report observations.
use crate::{
    core::OperationContext,
    error::{CoreError, Result},
    storage::{self, Store},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};
#[path = "harness_jobs.rs"]
mod jobs;
#[path = "harness_launch.rs"]
mod launch;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRole {
    Worker,
    Executor,
    Editor,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanIntent {
    DirectHuman,
    None,
}

/// Only trusted host code can issue this record. There is deliberately no
/// `harness.grant` Core operation that a model can invoke with JSON flags.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessGrant {
    pub scope_id: String,
    pub chain_id: Option<String>,
    pub turn_id: String,
    pub human_message_id: Option<String>,
    pub role: HarnessRole,
    pub human_intent: HumanIntent,
    pub allowed_calls: Vec<String>,
    pub object_id: Option<String>,
    pub goal_round: Option<u64>,
    pub max_goal_rounds: Option<u64>,
    pub wait_timeout_ms: Option<u64>,
    pub launch: Option<LaunchIdentity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchIdentity {
    pub call_id: String,
    pub native_name: String,
    pub arguments_sha256: String,
}

pub(crate) async fn issue(
    store: &Store,
    auth: &OperationContext,
    grant: HarnessGrant,
) -> Result<Value> {
    if auth.actor.is_empty() || !auth.allow_write || grant.role == HarnessRole::Editor {
        return Err(CoreError::forbidden(
            "trusted non-Editor host grant required",
        ));
    }
    if grant.scope_id.trim().is_empty() || grant.allowed_calls.is_empty() {
        return Err(CoreError::invalid("grant scope and allowed calls required"));
    }
    if auth.chain_id.is_some() && grant.chain_id != auth.chain_id {
        return Err(CoreError::forbidden("trusted turn chain mismatch"));
    }
    if grant.turn_id.trim().is_empty()
        || (grant.human_intent == HumanIntent::DirectHuman
            && grant
                .human_message_id
                .as_deref()
                .is_none_or(|s| s.trim().is_empty()))
    {
        return Err(CoreError::invalid(
            "trusted turn and direct human source identity required",
        ));
    }
    let id = storage::uid("harness_grant");
    let mut tx = store.begin().await?;
    storage::put_object(
        &mut tx,
        "harness_grant",
        &id,
        &json!({"actor":auth.actor,"grant":grant}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"grant_id":id}))
}

pub(crate) fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| CoreError::invalid(format!("{k} must be nonempty text")))
}
fn integer(v: &Value, k: &str) -> Result<u64> {
    v[k].as_u64()
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(|| CoreError::invalid(format!("{k} must be a safe unsigned integer")))
}
pub(crate) async fn latest(conn: &mut SqliteConnection, id: &str) -> Result<Value> {
    let row=sqlx::query("SELECT id FROM objects WHERE kind='harness_revision' AND json_extract(data,'$.object_id')=? ORDER BY CAST(json_extract(data,'$.revision') AS INTEGER) DESC LIMIT 1")
        .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(||CoreError::new("not_found","lifecycle object not found",404))?;
    let record: String = row.try_get("id")?;
    storage::get_object(conn, &record, Some("harness_revision")).await
}
pub(crate) async fn owned(
    conn: &mut SqliteConnection,
    actor: &str,
    scope: &str,
    id: &str,
) -> Result<Value> {
    let root = storage::get_object(conn, id, Some("harness_object")).await?;
    if root["actor"] != actor || root["scope_id"] != scope {
        return Err(CoreError::forbidden("lifecycle object outside actor scope"));
    }
    latest(conn, id).await
}
pub(crate) async fn verify_collection(
    conn: &mut SqliteConnection,
    actor: &str,
    object_id: &str,
    receipt_id: &str,
) -> Result<Value> {
    let root = storage::get_object(conn, object_id, Some("harness_object")).await?;
    if root["actor"] != actor
        || !matches!(
            root["object_kind"].as_str(),
            Some("job" | "workflow" | "peer")
        )
    {
        return Err(CoreError::forbidden(
            "collection outside actor activity scope",
        ));
    }
    let receipt = storage::get_object(conn, receipt_id, Some("harness_event")).await?;
    if receipt["actor"] != actor
        || receipt["native_name"] != "job_output"
        || receipt["result"]["object_id"] != object_id
    {
        return Err(CoreError::forbidden(
            "receipt is not a collection of this activity",
        ));
    }
    let output = receipt["result"]["value"]["text"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("collection text missing"))?;
    let digest = storage::sha256(output.as_bytes());
    if receipt["result"]["collection"]["text_digest"] != digest {
        return Err(CoreError::new(
            "integrity_error",
            "collection digest mismatch",
            500,
        ));
    }
    let current = latest(conn, object_id).await?;
    Ok(
        json!({"lifecycle_object_id":object_id,"collection_receipt_id":receipt_id,"collection_digest":digest,"external_id":current["external_id"],"chain_id":root["chain_id"]}),
    )
}
pub(crate) async fn save(conn: &mut SqliteConnection, state: &Value) -> Result<()> {
    storage::put_object(
        conn,
        "harness_revision",
        &storage::uid("harness_revision"),
        state,
    )
    .await
}
async fn ids(
    conn: &mut SqliteConnection,
    actor: &str,
    scope: &str,
    kind: &str,
) -> Result<Vec<String>> {
    let rows=sqlx::query("SELECT id FROM objects WHERE kind='harness_object' AND json_extract(data,'$.actor')=? AND json_extract(data,'$.scope_id')=? AND json_extract(data,'$.object_kind')=? ORDER BY rowid")
        .bind(actor).bind(scope).bind(kind).fetch_all(conn).await?;
    rows.iter()
        .map(|r| r.try_get("id").map_err(CoreError::from))
        .collect()
}
fn human(g: &HarnessGrant) -> Result<()> {
    if g.human_intent != HumanIntent::DirectHuman {
        return Err(CoreError::forbidden("direct human grant required"));
    }
    Ok(())
}
fn native_goal_revision(s: &Value) -> Result<u64> {
    match s.get("native_revision") {
        Some(_) => integer(s, "native_revision"),
        None => integer(s, "revision"),
    }
}
fn goal_value(s: &Value) -> Value {
    let revision = s.get("native_revision").unwrap_or(&s["revision"]);
    let mut goal = json!({"id":s["object_id"],"revision":revision,"objective":s["objective"],"phase":s["phase"],"roundsStarted":s["roundsStarted"],"maxGoalRounds":s["maxGoalRounds"]});
    if s["maxGoalRounds"].is_null() {
        goal.as_object_mut().unwrap().remove("maxGoalRounds");
    }
    if !s["blockedReason"].is_null() {
        goal["blockedReason"] = s["blockedReason"].clone();
    }
    json!({"goal":goal,"activation":s["activation"]})
}
async fn current_goal(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    scope: &str,
) -> Result<Option<Value>> {
    let all = ids(conn, &a.actor, scope, "goal").await?;
    match all.last() {
        Some(id) => {
            let state = owned(conn, &a.actor, scope, id).await?;
            Ok((state["cleared"] != true).then_some(state))
        }
        None => Ok(None),
    }
}
fn update_goal(g: &HarnessGrant, s: &mut Value, args: &Value) -> Result<()> {
    let action = text(args, "action")?;
    match action {
        "edit" => {
            human(g)?;
            if let Some(objective) = args.get("objective") {
                if objective.as_str().is_none_or(|s| s.trim().is_empty()) {
                    return Err(CoreError::invalid("objective must be nonempty text"));
                }
                s["objective"] = objective.clone();
            }
            if args.get("max_goal_rounds").is_some() {
                s["maxGoalRounds"] = json!(cap(g, args)?);
            }
            if args.get("objective").is_none() && args.get("max_goal_rounds").is_none() {
                return Err(CoreError::invalid(
                    "edit requires objective or max_goal_rounds",
                ));
            }
            s["blockedCount"] = json!(0);
            s["blockedCondition"] = Value::Null;
        }
        "pause" => {
            human(g)?;
            s["phase"] = json!("paused");
            s["activation"] = json!("disarmed");
            s["stop_reason"] = json!("human_pause");
        }
        "resume" => {
            human(g)?;
            if s["phase"] == "complete" {
                return Err(CoreError::conflict("completed goal cannot resume"));
            }
            if integer(s, "roundsStarted")? != integer(s, "roundsFinished")? {
                return Err(CoreError::conflict(
                    "unfinished goal round requires trusted interruption or outcome reconciliation before resume",
                ));
            }
            if s["maxGoalRounds"]
                .as_u64()
                .is_some_and(|cap| s["roundsStarted"].as_u64().unwrap_or(0) >= cap)
            {
                return Err(CoreError::conflict(
                    "round cap exhausted; human must edit cap before resuming",
                ));
            }
            s["phase"] = json!("active");
            s["activation"] = json!("armed");
            s["blockedCount"] = json!(0);
            s["blockedCondition"] = Value::Null;
            s["blockedReason"] = Value::Null;
            s["stop_reason"] = Value::Null;
        }
        "complete" => {
            continuation(g, s)?;
            s["phase"] = json!("complete");
            s["activation"] = json!("disarmed");
        }
        "blocked" => {
            continuation(g, s)?;
            let reason = text(args, "blocked_reason")?;
            if integer(s, "blockedCount")? < 3 || s["blockedCondition"] != reason {
                return Err(CoreError::conflict(
                    "same blocking condition must persist for at least three finished rounds",
                ));
            }
            s["phase"] = json!("blocked");
            s["activation"] = json!("disarmed");
            s["blockedReason"] = json!({"code":"persistent_condition","message":reason});
        }
        _ => return Err(CoreError::invalid("unsupported goal action")),
    }
    Ok(())
}
async fn observe_goal(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    if let Some(done) = replay(conn, a, p, "harness.observe").await? {
        return Ok(done);
    }
    let scope = text(p, "scope_id")?;
    let id = text(p, "object_id")?;
    let mut state = owned(conn, &a.actor, scope, id).await?;
    state["native_revision"] = json!(native_goal_revision(&state)?);
    if state["object_kind"] != "goal"
        || state["cleared"] == true
        || integer(p, "expected_revision")? != integer(&state, "revision")?
    {
        return Err(CoreError::conflict("lifecycle revision conflict"));
    }
    let observation = &p["observation"];
    let name = text(observation, "kind")?;
    let g = authorize(conn, a, p, name, Some(id), true).await?;
    match name {
        "goal.disarm" => {
            state["activation"] = json!("disarmed");
            state["stop_reason"] = json!("host_disarmed");
        }
        "goal.round.start" => {
            let next = integer(&state, "roundsStarted")? + 1;
            if state["phase"] != "active"
                || state["activation"] != "armed"
                || state["maxGoalRounds"]
                    .as_u64()
                    .is_some_and(|cap| next > cap)
                || g.goal_round != Some(next)
                || integer(&state, "roundsFinished")? + 1 != next
            {
                return Err(CoreError::conflict(
                    "goal round is not armed, finished, exact, or within cap",
                ));
            }
            state["roundsStarted"] = json!(next);
        }
        "goal.round.interrupted" => {
            let round = integer(&state, "roundsStarted")?;
            if round == 0
                || g.goal_round != Some(round)
                || integer(&state, "roundsFinished")? + 1 != round
            {
                return Err(CoreError::conflict(
                    "interruption must identify the exact unfinished goal round",
                ));
            }
            let reason = text(observation, "reason")?;
            state["roundsFinished"] = json!(round);
            state["activation"] = json!("disarmed");
            state["stop_reason"] = json!("interrupted_round");
            state["blockedCount"] = json!(0);
            state["blockedCondition"] = Value::Null;
            if state["interrupted_rounds"].is_null() {
                state["interrupted_rounds"] = json!([]);
            }
            state["interrupted_rounds"]
                .as_array_mut()
                .unwrap()
                .push(json!({"round":round,"reason":reason,"event_id":event_key(a,p)?}));
            state["round_receipts"]
                .as_array_mut()
                .unwrap()
                .push(json!(event_key(a, p)?));
        }
        "goal.round.finish" => {
            let round = integer(&state, "roundsStarted")?;
            if round == 0
                || g.goal_round != Some(round)
                || integer(&state, "roundsFinished")? + 1 != round
            {
                return Err(CoreError::conflict(
                    "goal finish must identify the exact unfinished round",
                ));
            }
            let outcome = text(observation, "outcome")?;
            if outcome == "blocked" {
                let condition = text(observation, "condition")?;
                state["blockedCount"] = json!(if state["blockedCondition"] == condition {
                    integer(&state, "blockedCount")? + 1
                } else {
                    1
                });
                state["blockedCondition"] = json!(condition);
            } else if outcome == "progress" {
                state["blockedCount"] = json!(0);
                state["blockedCondition"] = Value::Null;
            } else {
                return Err(CoreError::invalid(
                    "round outcome must be progress or blocked",
                ));
            }
            state["roundsFinished"] = json!(round);
            state["round_receipts"]
                .as_array_mut()
                .unwrap()
                .push(json!(event_key(a, p)?));
            if state["maxGoalRounds"]
                .as_u64()
                .is_some_and(|cap| round >= cap)
            {
                state["activation"] = json!("disarmed");
                state["stop_reason"] = json!("max_goal_rounds");
            }
        }
        _ => return Err(CoreError::invalid("unknown goal observation")),
    }
    state["revision"] = json!(integer(&state, "revision")? + 1);
    save(conn, &state).await?;
    receipt(conn, a, p, "harness.observe", &state, goal_value(&state)).await
}
fn cap(g: &HarnessGrant, args: &Value) -> Result<Option<u64>> {
    let approved = g.max_goal_rounds;
    if approved.is_some_and(|n| n == 0 || n > 9_007_199_254_740_991) {
        return Err(CoreError::invalid(
            "host round cap must be a positive safe integer",
        ));
    }
    let requested = if args.get("max_goal_rounds").is_some() {
        Some(integer(args, "max_goal_rounds")?)
    } else {
        approved
    };
    if requested == Some(0) || requested.zip(approved).is_some_and(|(a, b)| a > b) {
        return Err(CoreError::forbidden("round cap exceeds Harness grant"));
    }
    Ok(requested)
}
fn continuation(g: &HarnessGrant, s: &Value) -> Result<()> {
    if g.human_intent == HumanIntent::DirectHuman {
        return Ok(());
    }
    if (s["activation"] != "armed" && s["stop_reason"] != "max_goal_rounds")
        || g.goal_round != s["roundsStarted"].as_u64()
        || g.goal_round == Some(0)
    {
        return Err(CoreError::forbidden(
            "exact active goal round grant required",
        ));
    }
    Ok(())
}
async fn goal_call(conn: &mut SqliteConnection, a: &OperationContext, p: &Value) -> Result<Value> {
    let name = text(p, "name")?;
    let scope = text(p, "scope_id")?;
    let args = &p["arguments"];
    if name == "get_goal" {
        return Ok(
            json!({"value":match current_goal(conn,a,scope).await?{Some(s)=>goal_value(&s),None=>json!({"goal":null})}}),
        );
    }
    if let Some(done) = replay(conn, a, p, "harness.call").await? {
        return Ok(done);
    }
    if name == "create_goal" {
        let g = authorize(conn, a, p, name, None, false).await?;
        human(&g)?;
        if current_goal(conn, a, scope)
            .await?
            .is_some_and(|s| s["phase"] != "complete")
        {
            return Err(CoreError::conflict("unfinished goal already exists"));
        }
        let objective = text(args, "objective")?;
        let max = cap(&g, args)?;
        let id = storage::uid("goal");
        storage::put_object(
            conn,
            "harness_object",
            &id,
            &json!({"actor":a.actor,"scope_id":scope,"object_kind":"goal"}),
        )
        .await?;
        let state = json!({"object_id":id,"object_kind":"goal","revision":1,"native_revision":1,"objective":objective,"phase":"active","activation":"armed","roundsStarted":0,"roundsFinished":0,"maxGoalRounds":max,"blockedCount":0,"blockedCondition":null,"round_receipts":[]});
        save(conn, &state).await?;
        return receipt(conn, a, p, "harness.call", &state, goal_value(&state)).await;
    }
    let id = text(args, "goal_id")?;
    if current_goal(conn, a, scope)
        .await?
        .is_none_or(|s| s["object_id"] != id)
    {
        return Err(CoreError::conflict(
            "goal is no longer current in this scope",
        ));
    }
    let mut state = owned(conn, &a.actor, scope, id).await?;
    if state["object_kind"] != "goal" || integer(args, "revision")? != native_goal_revision(&state)?
    {
        return Err(CoreError::conflict("goal revision conflict"));
    }
    let g = authorize(conn, a, p, name, Some(id), false).await?;
    if name == "clear_goal" {
        human(&g)?;
        state["cleared"] = json!(true);
        state["activation"] = json!("disarmed");
        state["stop_reason"] = json!("human_clear");
    } else {
        update_goal(&g, &mut state, args)?;
    }
    state["native_revision"] = json!(
        native_goal_revision(&state)?
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or_else(|| CoreError::conflict("native goal revision exhausted"))?
    );
    state["revision"] = json!(integer(&state, "revision")? + 1);
    save(conn, &state).await?;
    let value = if name == "clear_goal" {
        json!({"cleared":{"id":id,"revision":native_goal_revision(&state)?}})
    } else {
        goal_value(&state)
    };
    receipt(conn, a, p, "harness.call", &state, value).await
}
async fn authorize(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    name: &str,
    id: Option<&str>,
    executor: bool,
) -> Result<HarnessGrant> {
    if !a.allow_write {
        return Err(CoreError::forbidden("write authorization required"));
    }
    let grant_id = text(p, "grant_id")?;
    let record = storage::get_object(conn, grant_id, Some("harness_grant")).await?;
    let g: HarnessGrant = serde_json::from_value(record["grant"].clone())?;
    if record["actor"] != a.actor
        || g.scope_id != text(p, "scope_id")?
        || !g.allowed_calls.iter().any(|s| s == name)
        || g.role
            != if executor {
                HarnessRole::Executor
            } else {
                HarnessRole::Worker
            }
        || id.is_some_and(|id| g.object_id.as_deref() != Some(id))
    {
        return Err(CoreError::forbidden(
            "lifecycle grant scope or role mismatch",
        ));
    }
    let used = format!("harness_used_{}", storage::sha256(grant_id.as_bytes()));
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM objects WHERE id=?")
        .bind(used)
        .fetch_optional(conn)
        .await?;
    if exists.is_some() {
        return Err(CoreError::conflict("lifecycle grant already consumed"));
    }
    Ok(g)
}
fn event_key(a: &OperationContext, p: &Value) -> Result<String> {
    Ok(format!(
        "harness_event_{}",
        storage::digest(&json!([
            a.actor,
            text(p, "scope_id")?,
            text(p, "event_id")?
        ]))?
    ))
}
async fn replay(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    op: &str,
) -> Result<Option<Value>> {
    let id = event_key(a, p)?;
    match storage::get_object(conn, &id, Some("harness_event")).await {
        Ok(record) => {
            if record["fingerprint"] != storage::digest(&json!([op, p]))? {
                return Err(CoreError::conflict(
                    "lifecycle event id reused with different input",
                ));
            }
            let mut result = record["result"].clone();
            if result.get("execution_permit").is_some() {
                result["execution_permit"] = json!(false);
            }
            result["replayed"] = json!(true);
            Ok(Some(result))
        }
        Err(e) if e.code == "not_found" => Ok(None),
        Err(e) => Err(e),
    }
}
async fn receipt(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    op: &str,
    state: &Value,
    value: Value,
) -> Result<Value> {
    receipt_extra(conn, a, p, op, state, value, json!({})).await
}
async fn receipt_extra(
    conn: &mut SqliteConnection,
    a: &OperationContext,
    p: &Value,
    op: &str,
    state: &Value,
    value: Value,
    extra: Value,
) -> Result<Value> {
    let id = event_key(a, p)?;
    let mut result = json!({"value":value,"object_id":state["object_id"],"revision":state["revision"],"receipt_id":id,"replayed":false});
    result.as_object_mut().unwrap().extend(
        extra
            .as_object()
            .ok_or_else(|| CoreError::invalid("invalid lifecycle receipt extension"))?
            .clone(),
    );
    if p["name"] == "job_output" {
        result["collection"] = state["last_collection"].clone();
    }
    if p["name"] == "job_kill" && state["status"] == "stopping" {
        result["executor_request"] = json!({"request_id":state["cancel_request_id"],"action":"cancel","executor":state["executor"],"reason":state["cancel_reason"]});
    }
    storage::put_object(conn,"harness_event",&id,&json!({"actor":a.actor,"scope_id":p["scope_id"],"grant_id":p["grant_id"],"native_name":p["name"],"observation_kind":p["observation"]["kind"],"fingerprint":storage::digest(&json!([op,p]))?,"result":result})).await?;
    let used = format!(
        "harness_used_{}",
        storage::sha256(text(p, "grant_id")?.as_bytes())
    );
    storage::put_object(conn, "harness_grant_use", &used, &json!({"event_id":id})).await?;
    Ok(result)
}
pub(crate) async fn operate(
    store: &Store,
    op: &str,
    a: &OperationContext,
    p: &Value,
) -> Result<Value> {
    let scope = text(p, "scope_id")?;
    if op == "harness.call" && p["name"] == "job_output" && p["arguments"]["wait"] == true {
        jobs::wait(store, a, p).await?;
    }
    let mut tx = store.begin().await?;
    let result = match op {
        "harness.get" => {
            let id=text(p,"object_id")?;
            let mut state=owned(&mut tx,&a.actor,scope,id).await?;
            if let Some(probe)=p.get("adoption_probe") {
                if !probe.is_object(){return Err(CoreError::invalid("adoption_probe must be an object"));}
                state["adoption_probe"]=jobs::adoption_probe(&mut tx,a,id,probe,false).await?;
            }
            state
        },
        "harness.observe" => match p["observation"]["kind"].as_str().unwrap_or("") {
            "job.intent" | "workflow.intent" | "peer.intent" | "job.claim" | "job.attach"
            | "job.recover" | "job.reconcile" | "job.remove" => {
                launch::observe(&mut tx, a, p).await?
            }
            "job.register" | "workflow.register" | "peer.register" => {
                jobs::register(&mut tx, a, p).await?
            }
            kind if kind.starts_with("goal.") => observe_goal(&mut tx, a, p).await?,
            _ => jobs::observe(&mut tx, a, p).await?,
        },
        "harness.call" => {
            let name = text(p, "name")?;
            if !p["arguments"].is_object() {
                return Err(CoreError::invalid("arguments must be an object"));
            }
            match name {
                "get_goal" | "create_goal" | "update_goal" | "clear_goal" => goal_call(&mut tx, a, p).await?,
                "job_output" | "job_list" | "job_kill" => jobs::call(&mut tx, a, p).await?,
                _ => return Err(CoreError::invalid("unsupported lifecycle native tool")),
            }
        }
        _ => return Err(CoreError::invalid("unsupported lifecycle operation")),
    };
    tx.commit().await?;
    if op == "harness.observe" || result.get("receipt_id").is_some() {
        jobs::notify();
    }
    Ok(
        json!({"ok":true,"operation":op,"context":{"actor":a.actor,"chain_id":a.chain_id},"evidence":{"provider_called":false,"lifecycle_owner":"sic"},"result":result}),
    )
}
