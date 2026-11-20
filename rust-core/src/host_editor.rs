//! Durable external Editor claims. Core owns scheduling; Harness owns execution.
use super::*;
pub(super) fn fresh(agent: &Agent, job: &Job) -> bool {
    job.base_cut == agent.cut_seq
        && job.host_editor["control_epoch"] == agent.control_epoch
        && job.host_editor["input_epoch"] == agent.input_epoch
        && agent.run_state == "running"
        && matches!(job.status.as_str(), "open" | "inflight" | "ready")
}
fn host_view(agent: &Agent, job: &Job, editor: Option<&str>) -> Result<Value> {
    let assigned = editor
        .and_then(|editor| {
            job.assignments
                .as_array()?
                .iter()
                .find(|row| row["editor"] == editor)
        })
        .map(|row| row["unit_ids"].clone())
        .unwrap_or(Value::Null);
    Ok(
        json!({"job_id":job.job_id,"source_id":job.source_id,"source_digest":storage::digest(&job.source_snapshot)?,
 "source_snapshot":job.source_snapshot,"editable_units":job.source_snapshot["units"],"units":job.source_snapshot["units"],"version":job.candidate_revision,"assignments":job.assignments,"assigned_unit_ids":assigned,
 "candidate_revision":job.candidate_revision,"base_cut":job.base_cut,"context_id":job.context_id,"level":job.level,"status":job.status,
 "edit_capabilities":{"ops":["KEEP","REPLACE","COMPRESS","DELETE","ADD","REORDER","FLAG"],"pvi":["P","V","I"],"required_host_bindings":{"V":"core-same-level-source-group-v1"}},
 "host_editor":job.host_editor,"editor_claims":job.editor_claims,"responses":job.responses,"fresh":fresh(agent,job),
 "editor_instructions":{"task_goal":job.frozen_editor_config["task_goal"],"task_constraints":job.frozen_editor_config["task_constraints"],"editor_prompt":job.frozen_editor_config["editor_prompt"]},
 "current_cut":agent.cut_seq,"current_control_epoch":agent.control_epoch,"current_input_epoch":agent.input_epoch}),
    )
}
impl Core {
    pub(super) async fn dispatch_host_editor(
        &self,
        conn: &mut SqliteConnection,
        agent: &mut Agent,
        auth: &OperationContext,
        op: &str,
        p: &Value,
        replayed: &mut bool,
    ) -> Result<Value> {
        if op == "edit.host.prepare" {
            let source = required_str(p, "source_id")?;
            if let Some(job) = agent.jobs.values().find(|job| job.source_id == source) {
                if !job.host_editor.is_object() {
                    return Err(CoreError::conflict(
                        "source already has a different Editor runner",
                    ));
                }
                if let Some(assignments) = p.get("assignments") {
                    if assignments != &job.assignments {
                        return Err(CoreError::conflict(
                            "source already has different Editor assignments",
                        ));
                    }
                }
                *replayed = true;
                return host_view(agent, job, None);
            }
            let prepared = self
                .prepare_with_profiles(conn, agent, p, false, false)
                .await?;
            let id = required_str(&prepared, "job_id")?;
            let epoch = json!({"runner":"host","control_epoch":agent.control_epoch,"input_epoch":agent.input_epoch,"attempts":{}});
            agent.jobs.get_mut(id).expect("prepared job").host_editor = epoch;
            return host_view(agent, agent.jobs.get(id).expect("prepared job"), None);
        }
        let id = required_str(p, "job_id")?.to_owned();
        let job = agent
            .jobs
            .get(&id)
            .cloned()
            .ok_or_else(|| CoreError::invalid("job not found"))?;
        if !job.host_editor.is_object() {
            return Err(CoreError::forbidden(
                "job is not owned by the host Editor runner",
            ));
        }
        // Read actual backing objects on every retry, including completed submissions.
        source_snapshot(conn, agent, &job.source_id, None).await?;
        let editor = p.get("editor").and_then(Value::as_str);
        if op == "edit.host.get" {
            return host_view(agent, &job, editor);
        }
        let editor = required_str(p, "editor")?.to_owned();
        if !job
            .assignments
            .as_array()
            .into_iter()
            .flatten()
            .any(|row| row["editor"] == editor)
        {
            return Err(CoreError::forbidden("Editor is not assigned to this job"));
        }
        let claim = required_str(p, "claim_id")?;
        if claim.len() > 128 || claim.chars().any(char::is_control) {
            return Err(CoreError::invalid("claim_id must be bounded text"));
        }
        let attempt = &job.host_editor["attempts"][&editor];
        let last_recovery = &job.host_editor["recovered_attempts"][&editor];
        let prior_recoveries = job.host_editor["recovery_history"][&editor].as_array().into_iter().flatten();
        let recorded = std::iter::once(last_recovery).chain(prior_recoveries).find(|recorded| recorded["claim_id"] == claim);
        if op == "edit.host.reopen" {
            let reason = required_str(p, "reason")?;
            if p["confirm"] != true || reason.trim().is_empty() || reason.len() > 512 {
                return Err(CoreError::invalid("explicit confirmation and a bounded recovery reason required"));
            }
            if let Some(recorded) = recorded {
                if recorded["actor"] != auth.actor || recorded["recovery_reason"] != reason {
                    return Err(CoreError::conflict("host Editor recovery already recorded"));
                }
                *replayed = true;
                return host_view(agent, &job, Some(&editor));
            }
            if attempt["claim_id"] != claim || attempt["actor"] != auth.actor {
                return Err(CoreError::forbidden("host Editor recovery does not own this claim"));
            }
            if job.status != "failed" || attempt["status"] != "unknown" || job.candidate.is_some()
                || job.cb_id.is_some() || !job.editor_claims.is_empty() || job.claim_id.is_some()
                || !job.assignments.as_array().is_some_and(|rows| rows.len() == 1)
                || agent.worker_claim.is_some() || job.base_cut != agent.cut_seq || agent.run_state != "running"
                || job.host_editor["control_epoch"] != agent.control_epoch
                || job.host_editor["input_epoch"] != agent.input_epoch
            {
                return Err(CoreError::conflict("host Editor recovery requires a current failed single-Editor job with no candidate"));
            }
            let target = agent.jobs.get_mut(&id).expect("existing job");
            let mut recovered = attempt.clone();
            recovered["recovery_reason"] = json!(reason);
            recovered["recovered_at"] = json!(now());
            if last_recovery.is_object() {
                let history = &mut target.host_editor["recovery_history"][&editor];
                if history.is_null() {
                    *history = json!([]);
                }
                history.as_array_mut().ok_or_else(|| CoreError::invalid("archived Editor recovery history is invalid"))?
                    .push(last_recovery.clone());
            }
            target.host_editor["recovered_attempts"] = json!({(editor.clone()): recovered});
            target.host_editor["attempts"] = json!({});
            target.status = "open".into();
            return host_view(agent, agent.jobs.get(&id).expect("existing job"), Some(&editor));
        }
        if op == "edit.host.claim" {
            if recorded.is_some() {
                return Err(CoreError::conflict("a recovered Editor job requires a new claim identity"));
            }
            if attempt.is_object() {
                if attempt["claim_id"] != claim || attempt["actor"] != auth.actor {
                    return Err(CoreError::conflict(
                        "host Editor already claimed; no second dispatch is allowed",
                    ));
                }
                let mut result = host_view(agent, &job, Some(&editor))?;
                result["execute"] = json!(false);
                result["claim_replayed"] = json!(true);
                *replayed = true;
                return Ok(result);
            }
            if !fresh(agent, &job) || job.status != "open" {
                return Err(CoreError::conflict("host Editor plan is stale or not open"));
            }
            if agent.worker_claim.is_some() {
                return Err(CoreError::conflict("Worker is still running"));
            }
            let target = agent.jobs.get_mut(&id).expect("existing job");
            target.editor_claims.insert(editor.clone(), claim.into());
            if target
                .assignments
                .as_array()
                .is_some_and(|rows| rows.len() == 1)
            {
                target.status = "inflight".into();
                target.claim_id = Some(claim.into());
            }
            target.host_editor["attempts"][&editor] =
                json!({"claim_id":claim,"actor":auth.actor,"status":"claimed","claimed_at":now()});
            let mut result = host_view(
                agent,
                agent.jobs.get(&id).expect("existing job"),
                Some(&editor),
            )?;
            result["execute"] = json!(true);
            return Ok(result);
        }
        if attempt["claim_id"] != claim || attempt["actor"] != auth.actor {
            return Err(CoreError::forbidden(
                "host Editor completion does not own this claim",
            ));
        }
        if op == "edit.host.submit" {
            let fingerprint = storage::digest(&json!([
                p.get("ops").unwrap_or(&json!([])),
                p.get("pvi").unwrap_or(&json!({}))
            ]))?;
            if attempt["status"] == "completed" {
                if attempt["fingerprint"] != fingerprint {
                    return Err(CoreError::conflict(
                        "host Editor claim replay changed its delivery",
                    ));
                }
                *replayed = true;
                let mut result = host_view(agent, &job, Some(&editor))?;
                result["submission"] = attempt["submission"].clone();
                return Ok(result);
            }
            if attempt["status"] != "claimed"
                || job.editor_claims.get(&editor).map(String::as_str) != Some(claim)
            {
                return Err(CoreError::conflict(
                    "host Editor attempt is terminal or unknown",
                ));
            }
            if !matches!(job.status.as_str(), "open" | "inflight") {
                return Err(CoreError::conflict(
                    "host Editor job is terminal; a sibling completion cannot revive it",
                ));
            }
            if !fresh(agent, &job) {
                return Err(CoreError::conflict(
                    "host Editor completion is stale; the current cut is unchanged",
                ));
            }
            let target = agent.jobs.get_mut(&id).expect("existing job");
            target.editor_claims.remove(&editor);
            target.claim_id = None;
            target.status = "open".into();
            let submission=self.submit(conn,agent,&json!({"job_id":id,"editor":editor,"ops":p.get("ops").cloned().unwrap_or_else(||json!([])),"pvi":p.get("pvi").cloned().unwrap_or_else(||json!({}))})).await?;
            let target = agent.jobs.get_mut(&id).expect("existing job");
            target.host_editor["attempts"][&editor]["status"] = json!("completed");
            target.host_editor["attempts"][&editor]["fingerprint"] = json!(fingerprint);
            target.host_editor["attempts"][&editor]["submission"] = submission.clone();
            let mut result = host_view(
                agent,
                agent.jobs.get(&id).expect("existing job"),
                Some(&editor),
            )?;
            result["submission"] = submission;
            return Ok(result);
        }
        if op == "edit.host.fail" {
            let reason = required_str(p, "reason")?;
            if attempt["status"] == "unknown" {
                if attempt["reason"] != reason {
                    return Err(CoreError::conflict("failure replay differs"));
                }
                *replayed = true;
                return host_view(agent, &job, Some(&editor));
            }
            if attempt["status"] != "claimed" {
                return Err(CoreError::conflict("host Editor attempt already finished"));
            }
            let target = agent.jobs.get_mut(&id).expect("existing job");
            target.editor_claims.remove(&editor);
            target.claim_id = None;
            target.status = "failed".into();
            target.host_editor["attempts"][&editor]["status"] = json!("unknown");
            target.host_editor["attempts"][&editor]["reason"] = json!(reason);
            return host_view(
                agent,
                agent.jobs.get(&id).expect("existing job"),
                Some(&editor),
            );
        }
        Err(CoreError::invalid("unknown host Editor operation"))
    }
}
