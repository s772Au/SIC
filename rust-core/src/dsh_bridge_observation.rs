//! Session-scoped DSH observations, accounting and context access.
use super::*;

impl DshBridge {
    /// Return an actor-scoped, transactionally consistent status snapshot.
    /// It contains only bridge-owned identities, counts and lifecycle states;
    /// prompt bodies, tool arguments/results and provider text stay private.
    pub async fn session_status(
        &self,
        actor: &str,
        session: &str,
    ) -> Result<Value, DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let row = sqlx::query("SELECT s.chain_id,s.active_fingerprint,t.state,t.phase,t.error_code FROM dsh_bridge_sessions s LEFT JOIN dsh_bridge_turns t ON t.actor=s.actor AND t.session_key=s.session_key AND t.fingerprint=s.active_fingerprint WHERE s.actor=? AND s.session_key=?")
            .bind(actor).bind(session).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let Some(row) = row else {
            return Err(DshBridgeError {
                status: StatusCode::NOT_FOUND,
                code: "session_not_found",
                message: "DSH session not found",
            });
        };
        let active_fingerprint = row
            .try_get::<Option<String>, _>("active_fingerprint")
            .map_err(|_| DshBridgeError::internal())?;
        let retired: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM dsh_bridge_retirements WHERE actor=? AND session_key=?",
        )
        .bind(actor)
        .bind(session)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        let latest = sqlx::query(
            "SELECT fingerprint,state,phase,error_code,created,\
             json_extract(progress,'$.ingested_source_id') AS ingested_source_id,\
             json_extract(progress,'$.wb_source_id') AS wb_source_id,\
             json_extract(progress,'$.wb_id') AS wb_id,\
             json_extract(progress,'$.job_id') AS job_id,\
             json_extract(progress,'$.published_cut') AS published_cut,\
             json_extract(progress,'$.published_cb') AS published_cb,\
             json_extract(progress,'$.worker_rounds[#-1].attempt_id') AS last_worker_attempt_id,\
             CASE WHEN json_type(progress,'$.worker_rounds')='array'\
                  THEN json_array_length(progress,'$.worker_rounds') END AS worker_rounds_recorded,\
             CASE WHEN json_type(progress,'$.tool_completions')='array'\
                  THEN json_array_length(progress,'$.tool_completions') END AS tool_completions_recorded \
             FROM dsh_bridge_turns WHERE actor=? AND session_key=? ORDER BY rowid DESC LIMIT 1",
        )
        .bind(actor)
        .bind(session)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        let last_turn = latest
            .map(|last| -> Result<Value, DshBridgeError> {
                let string = |name: &str| {
                    last.try_get::<Option<String>, _>(name)
                        .map_err(|_| DshBridgeError::internal())
                };
                let count = |name: &str| {
                    last.try_get::<Option<i64>, _>(name)
                        .map_err(|_| DshBridgeError::internal())
                };
                Ok(json!({
                    "fingerprint":string("fingerprint")?,
                    "state":string("state")?,
                    "phase":string("phase")?,
                    "error_code":string("error_code")?,
                    "created_unix_seconds":last.try_get::<i64,_>("created").map_err(|_| DshBridgeError::internal())?,
                    "references":{
                        "ingested_source_id":string("ingested_source_id")?,
                        "wb_source_id":string("wb_source_id")?,
                        "wb_id":string("wb_id")?,
                        "editor_job_id":string("job_id")?,
                        "published_cut_id":string("published_cut")?,
                        "published_cb_id":string("published_cb")?,
                        "last_worker_attempt_id":string("last_worker_attempt_id")?
                    },
                    "worker_rounds_recorded":count("worker_rounds_recorded")?,
                    "tool_completions_recorded":count("tool_completions_recorded")?
                }))
            })
            .transpose()?;
        let issued_tasks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dsh_bridge_tool_tasks WHERE actor=? AND session_key=? AND task_state='issued'",
        )
        .bind(actor)
        .bind(session)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        // A tool-result continuation is a distinct bridge turn and does not
        // necessarily publish a CB. Preserve a separately labelled pointer to
        // the latest publication recorded by this DSH session.
        let published = sqlx::query("SELECT fingerprint,created,\
            json_extract(progress,'$.wb_id') AS wb_id, json_extract(progress,'$.job_id') AS job_id,\
            json_extract(progress,'$.published_cut') AS cut_id, json_extract(progress,'$.published_cb') AS cb_id \
            FROM dsh_bridge_turns WHERE actor=? AND session_key=? AND json_type(progress,'$.published_cb')='text' \
            ORDER BY rowid DESC LIMIT 1")
            .bind(actor).bind(session).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let last_publication = published.map(|row| -> Result<Value, DshBridgeError> {
            let field = |key| row.try_get::<Option<String>,_>(key).map_err(|_| DshBridgeError::internal());
            Ok(json!({"turn_fingerprint":field("fingerprint")?,"created_unix_seconds":row.try_get::<i64,_>("created").map_err(|_| DshBridgeError::internal())?,
                "wb_id":field("wb_id")?,"editor_job_id":field("job_id")?,"cut_id":field("cut_id")?,"cb_id":field("cb_id")?}))
        }).transpose()?;
        let mut status = json!({"chain_id":row.try_get::<Option<String>,_>("chain_id").map_err(|_|DshBridgeError::internal())?,
            "active":active_fingerprint.is_some(),"active_fingerprint":active_fingerprint,"retired":retired.is_some(),
            "state":row.try_get::<Option<String>,_>("state").map_err(|_|DshBridgeError::internal())?,
            "phase":row.try_get::<Option<String>,_>("phase").map_err(|_|DshBridgeError::internal())?,
            "error_code":row.try_get::<Option<String>,_>("error_code").map_err(|_|DshBridgeError::internal())?,
            "last_turn":last_turn,"last_publication":last_publication,"issued_host_tasks":issued_tasks});
        // This is a content fingerprint, not a monotonic sequence or a
        // guarantee that the provider physically reused its cache.
        let revision = storage::digest(&status).map_err(|_| DshBridgeError::internal())?;
        status["revision"] = json!(revision);
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(status)
    }

    /// The chain is resolved from the authenticated DSH session, never from a
    /// caller-supplied chain ID. Only grouped usage leaves the Core boundary.
    pub async fn session_accounting(
        &self,
        actor: &str,
        session: &str,
    ) -> Result<Value, DshBridgeError> {
        let status = self.session_status(actor, session).await?;
        let Some(chain) = status["chain_id"].as_str() else {
            return Ok(json!({"chain_id":null,"groups":[],"complete":false,
                "attempts_with_unknown_usage":0,"unresolved_attempts":0,"pending_worker_actions":0,
                "superseded_worker_actions":0,"priced_cost":null}));
        };
        let response = self
            .operation(actor, Some(chain), "accounting.get", json!({}), false)
            .await?;
        let result = &response["result"];
        if !result["groups"].is_array() {
            return Err(DshBridgeError::core_failure());
        }
        Ok(json!({"chain_id":chain,"groups":result["groups"],
            "complete":result["complete"],"attempts_with_unknown_usage":result["attempts_with_unknown_usage"],
            "unresolved_attempts":result["unresolved_attempts"],
            "pending_worker_actions":result["pending_worker_actions"].as_u64().unwrap_or(0),
            "superseded_worker_actions":result["superseded_worker_actions"].as_u64().unwrap_or(0),
            "priced_cost":null,"physical_cache_hit":"unknown"}))
    }

    /// Read only explicitly selected context records from the session's own
    /// authorized chain. Content is returned only on this on-demand path.
    pub async fn session_context(
        &self,
        actor: &str,
        session: &str,
        action: &str,
        params: &Value,
    ) -> Result<Value, DshBridgeError> {
        let fields = params
            .as_object()
            .ok_or_else(|| DshBridgeError::invalid("Context parameters must be an object"))?;
        let valid_fields =
            |allowed: &[&str]| fields.keys().all(|key| allowed.contains(&key.as_str()));
        let operation = match action {
            "timeline"
                if valid_fields(&["level", "kind", "offset", "limit"])
                    && fields
                        .get("level")
                        .and_then(Value::as_u64)
                        .is_some_and(|v| (1..=128).contains(&v))
                    && fields
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|v| matches!(v, "wb" | "cb"))
                    && fields.get("offset").and_then(Value::as_u64).is_some()
                    && fields
                        .get("limit")
                        .and_then(Value::as_u64)
                        .is_some_and(|v| (1..=100).contains(&v)) =>
            {
                "hierarchy.timeline"
            }
            "expand"
                if fields.len() == 1
                    && valid_fields(&["wb_id", "cb_id"])
                    && fields
                        .values()
                        .next()
                        .and_then(Value::as_str)
                        .is_some_and(valid_native_id) =>
            {
                "hierarchy.expand"
            }
            "page"
                if valid_fields(&["kind", "object_id", "offset", "limit"])
                    && fields.len() == 4
                    && fields
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|v| matches!(v, "wb" | "cb" | "ops"))
                    && fields
                        .get("object_id")
                        .and_then(Value::as_str)
                        .is_some_and(valid_native_id)
                    && fields.get("offset").and_then(Value::as_u64).is_some()
                    && fields
                        .get("limit")
                        .and_then(Value::as_u64)
                        .is_some_and(|v| (1..=8192).contains(&v)) =>
            {
                "memory.page"
            }
            "decision" if fields.is_empty() => "decision.get",
            "candidate"
                if fields.len() == 1
                    && fields
                        .get("job_id")
                        .and_then(Value::as_str)
                        .is_some_and(valid_native_id) =>
            {
                "edit.candidate.get"
            }
            _ => {
                return Err(DshBridgeError::invalid(
                    "Unsupported context query or invalid parameters",
                ));
            }
        };
        let status = self.session_status(actor, session).await?;
        let chain = status["chain_id"].as_str().ok_or_else(|| {
            DshBridgeError::conflict("chain_unavailable", "DSH session has no associated chain")
        })?;
        let response = self
            .runtime
            .operate(
                actor.to_owned(),
                Some(chain.to_owned()),
                operation.to_owned(),
                params.clone(),
                false,
            )
            .await
            .map_err(|code| match code.as_str() {
                "forbidden" => DshBridgeError {
                    status: StatusCode::FORBIDDEN,
                    code: "forbidden",
                    message: "Context record is not accessible on this chain",
                },
                "invalid" => DshBridgeError::invalid("Context record or parameters are invalid"),
                "conflict" => {
                    DshBridgeError::conflict("context_changed", "Context changed during the query")
                }
                _ => DshBridgeError::core_failure(),
            })?;
        Ok(json!({"action":action,"result":response["result"]}))
    }

    /// Add one human-authored decision item with compare-and-swap. The session
    /// gate and Core's in-flight check prevent editing underneath a model call.
    pub async fn session_decision_event(
        &self,
        actor: &str,
        session: &str,
        expected: u64,
        event_id: &str,
        kind: &str,
        text: &str,
    ) -> Result<Value, DshBridgeError> {
        if !valid_native_id(session)
            || !valid_native_id(event_id)
            || !matches!(kind, "work" | "candidate" | "overlooked")
            || text.trim().is_empty()
            || text.len() > 4096
        {
            return Err(DshBridgeError::invalid("Invalid decision event"));
        }
        let _permit = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.lock_session(actor, session),
        )
        .await
        .map_err(|_| {
            DshBridgeError::conflict("session_busy", "Wait for the current DSH request to finish")
        })??;
        let status = self.session_status(actor, session).await?;
        if status["active"] == true || status["retired"] == true {
            return Err(DshBridgeError::conflict(
                "session_busy",
                "The DSH session is not available for edits",
            ));
        }
        let chain = status["chain_id"].as_str().ok_or_else(|| {
            DshBridgeError::conflict("chain_unavailable", "DSH session has no associated chain")
        })?;
        let payload = json!({"expected_revision":expected,"events":[{"id":event_id,"kind":kind,"text":text,"evidence":[]}]});
        let response = self
            .runtime
            .operate(
                actor.to_owned(),
                Some(chain.to_owned()),
                "decision.update".into(),
                payload,
                false,
            )
            .await
            .map_err(|code| match code.as_str() {
                "conflict" => DshBridgeError::conflict(
                    "decision_changed",
                    "Decision revision changed or a model call is active",
                ),
                "invalid" => DshBridgeError::invalid("Decision event was rejected"),
                "forbidden" => DshBridgeError {
                    status: StatusCode::FORBIDDEN,
                    code: "forbidden",
                    message: "Decision workspace is not accessible",
                },
                _ => DshBridgeError::core_failure(),
            })?;
        Ok(response["result"].clone())
    }

    pub(super) async fn native_tool_outcome(
        &self,
        actor: &str,
        session: &str,
        call: &str,
    ) -> Result<Option<String>, DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let outcome = sqlx::query_scalar("SELECT outcome FROM dsh_bridge_tool_events WHERE actor=? AND session_key=? AND call_id=?")
            .bind(actor).bind(session).bind(call).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(outcome)
    }

    /// Record only an authenticated host's event identity and outcome, and
    /// only for an already-issued matching call. Never retain event bodies.
    pub async fn record_native_tool_event(
        &self,
        actor: &str,
        session: &str,
        call: &str,
        seq: i64,
        outcome: &str,
    ) -> Result<Value, DshBridgeError> {
        if !valid_native_id(session)
            || !valid_native_id(call)
            || seq < 0
            || !matches!(outcome, "ok" | "error" | "not_started" | "unknown")
        {
            return Err(DshBridgeError::invalid(
                "Invalid native event identity or outcome",
            ));
        }
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM dsh_bridge_tool_tasks WHERE actor=? AND session_key=? AND call_id=?",
        )
        .bind(actor)
        .bind(session)
        .bind(call)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DshBridgeError::internal())?;
        if exists.is_none() {
            return Err(DshBridgeError {
                status: StatusCode::NOT_FOUND,
                code: "tool_task_not_found",
                message: "No issued tool task matches this session and call",
            });
        }
        let inserted = sqlx::query("INSERT OR IGNORE INTO dsh_bridge_tool_events(actor,session_key,call_id,event_seq,outcome,observed_at) VALUES(?,?,?,?,?,?)")
            .bind(actor).bind(session).bind(call).bind(seq).bind(outcome).bind(now() as i64)
            .execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?.rows_affected() == 1;
        let existing = sqlx::query("SELECT event_seq,outcome FROM dsh_bridge_tool_events WHERE actor=? AND session_key=? AND call_id=?")
            .bind(actor).bind(session).bind(call).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let Some(existing) = existing else {
            return Err(DshBridgeError::conflict(
                "native_event_conflict",
                "Native event sequence belongs to another task",
            ));
        };
        if existing
            .try_get::<i64, _>("event_seq")
            .map_err(|_| DshBridgeError::internal())?
            != seq
            || existing
                .try_get::<String, _>("outcome")
                .map_err(|_| DshBridgeError::internal())?
                != outcome
        {
            return Err(DshBridgeError::conflict(
                "native_event_conflict",
                "Native event differs from the stored observation",
            ));
        }
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        self.deliver_native_observation(actor, session, call)
            .await?;
        Ok(json!({"recorded":true,"replayed":!inserted}))
    }

    async fn deliver_native_observation(
        &self,
        actor: &str,
        session: &str,
        call: &str,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let row = sqlx::query("SELECT e.event_seq,e.outcome,e.core_observed,t.task_id,s.chain_id \
            FROM dsh_bridge_tool_events e JOIN dsh_bridge_tool_tasks t ON t.actor=e.actor AND t.session_key=e.session_key AND t.call_id=e.call_id \
            JOIN dsh_bridge_sessions s ON s.actor=e.actor AND s.session_key=e.session_key \
            WHERE e.actor=? AND e.session_key=? AND e.call_id=?")
            .bind(actor).bind(session).bind(call).fetch_optional(&mut *tx).await.map_err(|_| DshBridgeError::internal())?
            .ok_or_else(|| DshBridgeError::conflict("native_event_missing", "Native event or task mapping is unavailable"))?;
        let seq: i64 = row
            .try_get("event_seq")
            .map_err(|_| DshBridgeError::internal())?;
        let outcome: String = row
            .try_get("outcome")
            .map_err(|_| DshBridgeError::internal())?;
        let task: String = row
            .try_get("task_id")
            .map_err(|_| DshBridgeError::internal())?;
        let chain: String = row
            .try_get("chain_id")
            .map_err(|_| DshBridgeError::internal())?;
        let observed: i64 = row
            .try_get("core_observed")
            .map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        if observed == 1 {
            return Ok(());
        }
        let event_id = format!(
            "dsh:{}",
            storage::digest(&json!([actor, session, seq]))
                .map_err(|_| DshBridgeError::internal())?
        );
        let response = self
            .operation(
                actor,
                Some(&chain),
                "tool.observe",
                json!({
                    "task_id":task,"claim_id":stable_tool_claim_id(actor,session,&task)?,
                    "event_id":event_id,"source":"dsh_tool_result","outcome":outcome
                }),
                false,
            )
            .await?;
        if response["result"]["recorded"] != true {
            return Err(DshBridgeError::core_failure());
        }
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        sqlx::query("UPDATE dsh_bridge_tool_events SET core_observed=1 WHERE actor=? AND session_key=? AND call_id=? AND event_seq=? AND outcome=?")
            .bind(actor).bind(session).bind(call).bind(seq).bind(outcome).execute(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())
    }

    pub(super) async fn reconcile_native_observations(
        &self,
        actor: &str,
        session: &str,
    ) -> Result<(), DshBridgeError> {
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let pending: Vec<String> = sqlx::query_scalar("SELECT call_id FROM dsh_bridge_tool_events WHERE actor=? AND session_key=? AND core_observed=0 ORDER BY event_seq LIMIT 257")
            .bind(actor).bind(session).fetch_all(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        for call in pending.iter().take(256) {
            self.deliver_native_observation(actor, session, call)
                .await?;
        }
        if pending.len() > 256 {
            return Err(DshBridgeError::conflict(
                "native_events_pending",
                "More native observations need reconciliation before this session continues",
            ));
        }
        Ok(())
    }

    /// A bounded, actor-scoped host projection; task arguments and results are
    /// deliberately excluded. A late observation does not rewrite prior CBs.
    pub async fn session_tool_events(
        &self,
        actor: &str,
        session: &str,
        before: Option<i64>,
        limit: i64,
    ) -> Result<Value, DshBridgeError> {
        self.session_status(actor, session).await?;
        if !(1..=100).contains(&limit) || before.is_some_and(|v| v <= 0) {
            return Err(DshBridgeError::invalid("Invalid tool event page bounds"));
        }
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| DshBridgeError::internal())?;
        let rows = sqlx::query("SELECT t.rowid AS cursor,t.call_id,t.task_id,t.tool_name,t.task_state,t.completion_outcome,t.created,e.event_seq,e.outcome,e.observed_at,e.core_observed \
            FROM dsh_bridge_tool_tasks t LEFT JOIN dsh_bridge_tool_events e \
            ON e.actor=t.actor AND e.session_key=t.session_key AND e.call_id=t.call_id \
            WHERE t.actor=? AND t.session_key=? AND t.rowid < ? ORDER BY t.rowid DESC LIMIT ?")
            .bind(actor).bind(session).bind(before.unwrap_or(i64::MAX)).bind(limit)
            .fetch_all(&mut *tx).await.map_err(|_| DshBridgeError::internal())?;
        let mut items = Vec::with_capacity(rows.len());
        let mut last = None;
        for row in rows {
            let cursor: i64 = row
                .try_get("cursor")
                .map_err(|_| DshBridgeError::internal())?;
            last = Some(cursor);
            let string = |key| {
                row.try_get::<Option<String>, _>(key)
                    .map_err(|_| DshBridgeError::internal())
            };
            let number = |key| {
                row.try_get::<Option<i64>, _>(key)
                    .map_err(|_| DshBridgeError::internal())
            };
            items.push(json!({"call_id":string("call_id")?,"task_id":string("task_id")?,
                "tool_name":string("tool_name")?,"task_state":string("task_state")?,"completion_outcome":string("completion_outcome")?,
                "created_unix_seconds":number("created")?,"native_event_seq":number("event_seq")?,
                "native_outcome":string("outcome")?,"core_observed":number("core_observed")?.map(|value| value == 1),"observed_unix_seconds":number("observed_at")?}));
        }
        let next = if items.len() == limit as usize {
            last
        } else {
            None
        };
        tx.commit().await.map_err(|_| DshBridgeError::internal())?;
        Ok(json!({"items":items,"next_before":next}))
    }
}

pub(super) async fn session_status_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let fields = match value.as_object().filter(|fields| {
        fields
            .keys()
            .all(|key| matches!(key.as_str(), "session_id" | "if_revision"))
    }) {
        Some(fields) => fields,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "A bounded session_id and optional revision are required",
            );
        }
    };
    let session = match fields
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
    {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "A bounded session_id is required",
            );
        }
    };
    let requested_revision = match fields.get("if_revision") {
        None => None,
        Some(Value::String(value))
            if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Some(value.as_str())
        }
        Some(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "if_revision must be a SHA-256 hex fingerprint",
            );
        }
    };
    match state.bridge.session_status(&actor, session).await {
        Ok(value)
            if requested_revision
                .is_some_and(|revision| value["revision"].as_str() == Some(revision)) =>
        {
            (
                StatusCode::OK,
                Json(json!({"unchanged":true,"revision":value["revision"]})),
            )
                .into_response()
        }
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

pub(super) async fn session_accounting_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let session = value
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control));
    let Some(session) = session else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "A bounded session_id is required",
        );
    };
    match state.bridge.session_accounting(&actor, session).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

pub(super) async fn native_tool_event_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| fields.len() == 4) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id, call_id, event_seq and outcome are required",
        );
    };
    let (Some(session), Some(call), Some(seq), Some(outcome)) = (
        fields.get("session_id").and_then(Value::as_str),
        fields.get("call_id").and_then(Value::as_str),
        fields.get("event_seq").and_then(Value::as_i64),
        fields.get("outcome").and_then(Value::as_str),
    ) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Native event fields have invalid types",
        );
    };
    match state
        .bridge
        .record_native_tool_event(&actor, session, call, seq, outcome)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

pub(super) async fn session_tool_events_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| {
        fields
            .keys()
            .all(|k| matches!(k.as_str(), "session_id" | "before" | "limit"))
    }) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid tool events page request",
        );
    };
    let Some(session) = fields
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| valid_native_id(s))
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "A bounded session_id is required",
        );
    };
    let before = match fields.get("before") {
        None => None,
        Some(v) => match v.as_i64() {
            Some(n) if n > 0 => Some(n),
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Invalid page cursor",
                );
            }
        },
    };
    let limit = match fields.get("limit") {
        None => 50,
        Some(v) => match v.as_i64() {
            Some(n) if (1..=100).contains(&n) => n,
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Invalid page size",
                );
            }
        },
    };
    match state
        .bridge
        .session_tool_events(&actor, session, before, limit)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

pub(super) async fn session_context_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| fields.len() == 3) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "session_id, action and params are required",
        );
    };
    let (Some(session), Some(action), Some(params)) = (
        fields
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|s| valid_native_id(s)),
        fields.get("action").and_then(Value::as_str),
        fields.get("params"),
    ) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid context query",
        );
    };
    match state
        .bridge
        .session_context(&actor, session, action, params)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}

pub(super) async fn session_decision_event_route(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let actor = match (state.resolve_actor)(&headers) {
        Ok(actor) if !actor.trim().is_empty() => actor,
        _ => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Host authentication required",
            );
        }
    };
    let value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "Request body must be JSON",
            );
        }
    };
    let Some(fields) = value.as_object().filter(|fields| fields.len() == 5) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid decision event request",
        );
    };
    let (Some(session), Some(revision), Some(event), Some(kind), Some(text)) = (
        fields.get("session_id").and_then(Value::as_str),
        fields.get("expected_revision").and_then(Value::as_u64),
        fields.get("event_id").and_then(Value::as_str),
        fields.get("kind").and_then(Value::as_str),
        fields.get("text").and_then(Value::as_str),
    ) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid decision event fields",
        );
    };
    match state
        .bridge
        .session_decision_event(&actor, session, revision, event, kind, text)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error.status, error.code, error.message),
    }
}
