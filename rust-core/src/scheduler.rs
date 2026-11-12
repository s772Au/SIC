//! Bounded, public-operation-only SIC Editor and hierarchy scheduler.
//!
//! Provider mode executes one `edit.run` per Editor job and halts on every
//! error. ProgramFixture mode is deterministic local materialization and must
//! never be reported as an LLM/provider acceptance. No provider call is retried
//! by this scheduler; Core's configured Editor correction bound is reserved
//! against the model-call budget before each `edit.run`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    core::{Core, OperationContext},
    error::CoreError,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditorMode {
    Provider,
    ProgramFixture,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerPolicy {
    pub context_id: String,
    pub min_sources_per_wb: usize,
    pub max_sources_per_wb: usize,
    pub max_units_per_wb: usize,
    /// For each level >= 2, the number of continuous lower-level CBs per WB.
    pub level_batch_sizes: BTreeMap<u32, usize>,
    pub max_level: u32,
    pub flush: bool,
    pub max_actions: usize,
    pub max_model_calls: usize,
    pub editor_mode: EditorMode,
    pub program_rules: Vec<String>,
}

impl Default for SchedulerPolicy {
    fn default() -> Self {
        Self {
            context_id: "main".into(),
            min_sources_per_wb: 1,
            max_sources_per_wb: 8,
            max_units_per_wb: 512,
            level_batch_sizes: BTreeMap::new(),
            max_level: 1,
            flush: false,
            max_actions: 128,
            max_model_calls: 32,
            editor_mode: EditorMode::Provider,
            program_rules: vec!["deduplicate_exact".into()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerFailure {
    pub operation: String,
    pub code: String,
    pub status: u16,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedBlock {
    pub level: u32,
    pub job_id: String,
    pub cb_id: String,
    pub cut_id: String,
    pub source_ids: Vec<String>,
    pub volatile_source_id: Option<String>,
    pub provider_called: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterleaveReceipt {
    pub wb_source_id: String,
    pub permit_id: String,
    pub cost: u64,
    pub remaining: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitDeclaration {
    pub code: String,
    pub required_action: String,
    pub context_id: String,
    pub level: u32,
    pub available: Option<usize>,
    pub required: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerReport {
    pub context_id: String,
    pub editor_mode: EditorMode,
    pub actions: usize,
    pub model_calls_observed: usize,
    pub model_calls_reserved_unknown: usize,
    pub published: Vec<PublishedBlock>,
    pub interleaves: Vec<InterleaveReceipt>,
    pub needs_flush: bool,
    pub wait_reason: Option<String>,
    pub wait: Option<WaitDeclaration>,
    pub failure: Option<SchedulerFailure>,
    pub completed: bool,
}

impl SchedulerReport {
    fn new(policy: &SchedulerPolicy) -> Self {
        Self {
            context_id: policy.context_id.clone(),
            editor_mode: policy.editor_mode,
            actions: 0,
            model_calls_observed: 0,
            model_calls_reserved_unknown: 0,
            published: Vec::new(),
            interleaves: Vec::new(),
            needs_flush: false,
            wait_reason: None,
            wait: None,
            failure: None,
            completed: false,
        }
    }
}

/// Run a bounded scheduling pass through public `Core::operate` operations.
/// A failed operation halts this pass; retry requires a fresh host invocation.
pub async fn schedule(
    core: &Core,
    context: &OperationContext,
    policy: &SchedulerPolicy,
) -> SchedulerReport {
    let mut runner = Runner {
        core,
        context,
        policy,
        report: SchedulerReport::new(policy),
        expected_cut: 0,
        max_corrections: 0,
        last_operation: None,
    };
    if let Err(error) = runner.run().await {
        let operation = runner
            .last_operation
            .clone()
            .unwrap_or_else(|| "scheduler".into());
        runner.report.failure = Some(SchedulerFailure {
            operation,
            code: error.code.into(),
            status: error.status,
            message: error.message,
        });
    }
    runner.report
}

struct Runner<'a> {
    core: &'a Core,
    context: &'a OperationContext,
    policy: &'a SchedulerPolicy,
    report: SchedulerReport,
    expected_cut: u64,
    max_corrections: u64,
    last_operation: Option<String>,
}

impl Runner<'_> {
    async fn run(&mut self) -> crate::error::Result<()> {
        validate_policy(self.policy)?;
        if !self.context.allow_write {
            return Err(CoreError::forbidden(
                "scheduler requires host write authorization",
            ));
        }
        if self.context.chain_id.as_deref().is_none_or(str::is_empty) {
            return Err(CoreError::invalid(
                "scheduler requires an authenticated chain context",
            ));
        }

        // chain.get is the public read-only snapshot containing both contexts
        // and Editor job states; hierarchy.get omits job state.
        let hierarchy = self.operate("chain.get", json!({})).await?;
        let contexts = hierarchy["result"]["contexts"]
            .as_object()
            .ok_or_else(|| CoreError::invalid("chain snapshot has no contexts"))?;
        if !contexts.contains_key(&self.policy.context_id) {
            return Err(CoreError::invalid("scheduler context does not exist"));
        }
        if self.policy.editor_mode == EditorMode::Provider {
            if !self.context.allow_model {
                return Err(CoreError::forbidden(
                    "provider scheduling requires host model authorization",
                ));
            }
            let config = self.operate("config.get", json!({})).await?;
            self.max_corrections =
                config["result"]["configuration"]["editor_feedback"]["max_corrections"]
                    .as_u64()
                    .unwrap_or(2)
                    .min(3);
        }
        let cut = self.operate("cut.get", json!({})).await?;
        self.expected_cut = cut["result"]["sequence"]
            .as_u64()
            .ok_or_else(|| CoreError::invalid("cut sequence missing"))?;

        self.schedule_first_order().await?;
        for level in 2..=self.policy.max_level {
            self.schedule_higher_level(level).await?;
        }
        self.report.completed = self.report.failure.is_none() && self.report.wait_reason.is_none();
        Ok(())
    }

    async fn schedule_first_order(&mut self) -> crate::error::Result<()> {
        loop {
            let hierarchy = self.operate("chain.get", json!({})).await?;
            let pending = pending_sources(&hierarchy, &self.policy.context_id, 1)?;
            if pending.is_empty() {
                return Ok(());
            }

            let open_wbs = self.timeline_rows(1, "wb").await?;
            let existing = open_wbs.iter().find(|row| {
                row["source_id"].as_str() == Some(&pending[0]) && row["cb_id"].is_null()
            });

            let source_id = if let Some(wb) = existing {
                wb["source_id"].as_str().unwrap().to_owned()
            } else {
                let mut selected = Vec::new();
                let mut units = 0_usize;
                for source_id in pending.iter().take(self.policy.max_sources_per_wb) {
                    let read = self
                        .operate("source.read", json!({"source_id":source_id,"group":true}))
                        .await?;
                    let source = &read["result"]["source"];
                    if source["open_calls"]
                        .as_array()
                        .is_some_and(|calls| !calls.is_empty())
                    {
                        self.report.wait_reason =
                            Some("pending source has unresolved native tool calls".into());
                        self.report.wait = Some(WaitDeclaration {
                            code: "unresolved_tool_calls".into(),
                            required_action: "host must close or reject the open native call"
                                .into(),
                            context_id: self.policy.context_id.clone(),
                            level: 1,
                            available: Some(pending.len()),
                            required: None,
                        });
                        return Ok(());
                    }
                    let count = source["units"].as_array().map_or(0, Vec::len);
                    if count > self.policy.max_units_per_wb && selected.is_empty() {
                        return Err(CoreError::new(
                            "block_limit",
                            "one pending source exceeds max_units_per_wb and cannot be split",
                            413,
                        ));
                    }
                    if units.saturating_add(count) > self.policy.max_units_per_wb {
                        break;
                    }
                    selected.push(source_id.clone());
                    units += count;
                }
                if selected.is_empty() {
                    return Ok(());
                }
                if selected.len() < self.policy.min_sources_per_wb && !self.policy.flush {
                    // F34: an early block is allowed only with a host-set,
                    // per-WB permit; the receipt is idempotent and charged
                    // once. Without budget the pass waits as before.
                    let wb_source_id = selected[0].clone();
                    let permit_id = format!("early:{wb_source_id}");
                    let receipt = match self
                        .operate(
                            "runtime.budget",
                            json!({"wb_source_id":wb_source_id,"permit_id":permit_id,"cost":1}),
                        )
                        .await
                    {
                        Ok(receipt) => receipt["result"].clone(),
                        Err(error)
                            if matches!(error.code, "budget_missing" | "budget_exhausted") =>
                        {
                            self.report.needs_flush = true;
                            self.report.wait_reason = Some(format!(
                                "pending batch is below min_sources_per_wb; explicit flush required (interleave {})",
                                error.code
                            ));
                            self.report.wait = Some(WaitDeclaration {
                                code: format!("interleave_{}", error.code),
                                required_action:
                                    "host must grant a per-WB budget or explicitly flush".into(),
                                context_id: self.policy.context_id.clone(),
                                level: 1,
                                available: Some(selected.len()),
                                required: Some(self.policy.min_sources_per_wb),
                            });
                            return Ok(());
                        }
                        Err(error) => return Err(error),
                    };
                    self.report.interleaves.push(InterleaveReceipt {
                        wb_source_id: required_string(&receipt, "wb_source_id")?.to_owned(),
                        permit_id: required_string(&receipt, "permit_id")?.to_owned(),
                        cost: receipt["cost"].as_u64().unwrap_or(0),
                        remaining: receipt["remaining"].as_u64().unwrap_or(0),
                    });
                }
                let source_id = if selected.len() > 1 {
                    let formed = self
                        .operate(
                            "wb.form",
                            json!({
                                "context_id":self.policy.context_id,"source_ids":selected
                            }),
                        )
                        .await?;
                    required_string(&formed["result"], "source_id")?.to_owned()
                } else {
                    selected[0].clone()
                };
                source_id
            };
            self.ensure_no_ambiguous_prior_job(&hierarchy, &source_id)?;
            self.edit_and_publish(1, &source_id).await?;
        }
    }

    async fn schedule_higher_level(&mut self, level: u32) -> crate::error::Result<()> {
        let batch = self.policy.level_batch_sizes[&level];
        let snapshot = self.operate("chain.get", json!({})).await?;
        let level_key = level.to_string();
        let levels = snapshot["result"]["contexts"][self.policy.context_id.as_str()]["levels"]
            .as_object()
            .ok_or_else(|| CoreError::invalid("chain context has no levels"))?;
        let lower_key = (level - 1).to_string();
        if !levels.contains_key(&lower_key) {
            return Ok(());
        }
        let level_exists = levels.contains_key(&level_key);
        let lower_rows = self.timeline_rows(level - 1, "cb").await?;
        let higher_rows = if level_exists {
            self.timeline_rows(level, "wb").await?
        } else {
            Vec::new()
        };
        let lower_ids: Vec<String> = lower_rows
            .iter()
            .map(|row| required_string(row, "id").map(str::to_owned))
            .collect::<crate::error::Result<_>>()?;
        let mut consumed = BTreeSet::new();
        for row in &higher_rows {
            if let Some(members) = row["members"].as_array() {
                for member in members.iter().filter_map(Value::as_str) {
                    consumed.insert(member.to_owned());
                }
            }
        }

        // Existing unpaired higher WBs are admitted work from an earlier
        // bounded pass. Resume only an untouched/open job; failed/ambiguous
        // provider outcomes are never reissued automatically.
        for row in higher_rows.iter().filter(|row| row["cb_id"].is_null()) {
            let source = required_string(row, "source_id")?;
            let hierarchy = self.operate("chain.get", json!({})).await?;
            self.ensure_no_ambiguous_prior_job(&hierarchy, source)?;
            self.edit_and_publish(level, source).await?;
        }

        let groups = consecutive_unconsumed_groups(&lower_ids, &consumed, batch, self.policy.flush);
        let scheduled_members: usize = groups.iter().map(Vec::len).sum();
        let available_members = lower_ids
            .iter()
            .filter(|id| !consumed.contains(*id))
            .count();
        if !self.policy.flush && scheduled_members < available_members {
            self.report.needs_flush = true;
            self.report.wait_reason.get_or_insert_with(|| {
                format!("level {level} has an unconsumed CB interval below its batch threshold; explicit flush required")
            });
            self.report.wait.get_or_insert(WaitDeclaration {
                code: "level_batch".into(),
                required_action: "host must explicitly flush the remaining lower-level interval"
                    .into(),
                context_id: self.policy.context_id.clone(),
                level,
                available: Some(available_members),
                required: Some(batch),
            });
        }
        for members in groups {
            let formed = self
                .operate(
                    "hierarchy.form",
                    json!({
                        "context_id":self.policy.context_id,"level":level,"cb_ids":members
                    }),
                )
                .await?;
            let source = required_string(&formed["result"], "source_id")?.to_owned();
            self.edit_and_publish(level, &source).await?;
        }
        Ok(())
    }

    async fn edit_and_publish(&mut self, level: u32, source_id: &str) -> crate::error::Result<()> {
        let hierarchy = self.operate("chain.get", json!({})).await?;
        let prior = find_job_for_source(&hierarchy, source_id)?;
        let (job_id, status, new_candidate) = if let Some((job_id, status)) = prior {
            if status == "failed" || status == "inflight" || status == "superseded" {
                return Err(CoreError::conflict(
                    "prior Editor job is failed or ambiguous; scheduler will not retry it",
                ));
            }
            (job_id, status, false)
        } else {
            let operation = match self.policy.editor_mode {
                EditorMode::Provider => "edit.prepare",
                EditorMode::ProgramFixture => "edit.program",
            };
            let mut payload = json!({"source_id":source_id});
            if self.policy.editor_mode == EditorMode::ProgramFixture {
                payload["rules"] = json!(self.policy.program_rules);
            }
            let prepared = self.operate(operation, payload).await?;
            (
                required_string(&prepared["result"], "job_id")?.to_owned(),
                if self.policy.editor_mode == EditorMode::ProgramFixture {
                    "ready".into()
                } else {
                    "open".into()
                },
                true,
            )
        };

        let mut provider_called = false;
        if self.policy.editor_mode == EditorMode::Provider && status == "open" {
            let cost = self.max_corrections as usize + 1;
            if self
                .report
                .model_calls_observed
                .saturating_add(self.report.model_calls_reserved_unknown)
                .saturating_add(cost)
                > self.policy.max_model_calls
            {
                self.last_operation = Some("scheduler.model_call_budget".into());
                return Err(CoreError::new(
                    "model_call_budget",
                    "worst-case Editor retries exceed max_model_calls",
                    429,
                ));
            }
            let run = match self.operate("edit.run", json!({"job_id":job_id})).await {
                Ok(value) => value,
                Err(error) => {
                    self.report.model_calls_reserved_unknown += cost;
                    return Err(error);
                }
            };
            provider_called = run["evidence"]["provider_called"]
                .as_bool()
                .unwrap_or(false);
            let attempts = run["result"]["attempts"].as_u64().unwrap_or(cost as u64) as usize;
            self.report.model_calls_observed += attempts;
            if attempts > cost || self.report.model_calls_observed > self.policy.max_model_calls {
                self.last_operation = Some("scheduler.model_call_budget".into());
                return Err(CoreError::new(
                    "model_call_budget",
                    "Core exceeded the reserved model-call budget",
                    500,
                ));
            }
        } else if self.policy.editor_mode == EditorMode::ProgramFixture && status != "ready" {
            return Err(CoreError::conflict(
                "program fixture job is not ready; no automatic retry",
            ));
        } else if self.policy.editor_mode == EditorMode::Provider
            && status != "ready"
            && !new_candidate
        {
            return Err(CoreError::conflict(
                "existing Editor job is not ready for publication",
            ));
        }

        let publish = self
            .operate(
                "cut.publish",
                json!({
                    "job_id":job_id,"expected_cut":self.expected_cut,
                    "idempotency_key":format!("scheduler-{job_id}")
                }),
            )
            .await?;
        self.expected_cut = self.expected_cut.saturating_add(1);
        self.report.published.push(PublishedBlock {
            level,
            job_id,
            cb_id: required_string(&publish["result"], "cb_id")?.to_owned(),
            cut_id: required_string(&publish["result"], "cut_id")?.to_owned(),
            source_ids: publish["result"]["source_ids"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            volatile_source_id: publish["result"]["volatile_source_id"]
                .as_str()
                .map(str::to_owned),
            provider_called,
        });
        Ok(())
    }

    fn ensure_no_ambiguous_prior_job(
        &self,
        hierarchy: &Value,
        source_id: &str,
    ) -> crate::error::Result<()> {
        if let Some((_, status)) = find_job_for_source(hierarchy, source_id)? {
            if status == "failed" || status == "inflight" || status == "superseded" {
                return Err(CoreError::conflict(
                    "prior Editor job has uncertain outcome; operator recovery is required",
                ));
            }
        }
        Ok(())
    }

    async fn operate(&mut self, operation: &str, payload: Value) -> crate::error::Result<Value> {
        if self.report.actions >= self.policy.max_actions {
            self.last_operation = Some("scheduler.max_actions".into());
            return Err(CoreError::new(
                "action_budget",
                "scheduler max_actions exhausted",
                429,
            ));
        }
        self.report.actions += 1;
        self.last_operation = Some(operation.into());
        self.core.operate(operation, self.context, &payload).await
    }

    async fn timeline_rows(&mut self, level: u32, kind: &str) -> crate::error::Result<Vec<Value>> {
        let mut rows = Vec::new();
        let mut offset = 0_u64;
        loop {
            let page = self
                .operate(
                    "hierarchy.timeline",
                    json!({"context_id":self.policy.context_id,"level":level,"kind":kind,"limit":100,"offset":offset}),
                )
                .await?;
            rows.extend(
                page["result"]["rows"]
                    .as_array()
                    .ok_or_else(|| CoreError::invalid("timeline rows missing"))?
                    .iter()
                    .cloned(),
            );
            let Some(next) = page["result"]["next_offset"].as_u64() else {
                break;
            };
            if next <= offset {
                return Err(CoreError::new(
                    "timeline_invalid",
                    "timeline pagination did not advance",
                    500,
                ));
            }
            offset = next;
        }
        Ok(rows)
    }
}

fn validate_policy(policy: &SchedulerPolicy) -> crate::error::Result<()> {
    if policy.context_id.trim().is_empty()
        || policy.min_sources_per_wb == 0
        || policy.max_sources_per_wb < policy.min_sources_per_wb
        || policy.max_units_per_wb == 0
        || policy.max_level == 0
        || policy.max_level > 128
        || policy.max_actions == 0
    {
        return Err(CoreError::invalid("invalid scheduler policy bounds"));
    }
    for level in 2..=policy.max_level {
        if policy
            .level_batch_sizes
            .get(&level)
            .is_none_or(|size| *size == 0)
        {
            return Err(CoreError::invalid(
                "each scheduled higher level requires a positive CB batch size",
            ));
        }
    }
    Ok(())
}

fn pending_sources(
    hierarchy: &Value,
    context_id: &str,
    level: u32,
) -> crate::error::Result<Vec<String>> {
    let key = level.to_string();
    let value = hierarchy["result"]["contexts"][context_id]["levels"][key.as_str()]["pending"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("context pending source list missing"))?;
    value
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| CoreError::invalid("pending source ID is invalid"))
        })
        .collect()
}

fn find_job_for_source(
    hierarchy: &Value,
    source_id: &str,
) -> crate::error::Result<Option<(String, String)>> {
    let jobs = hierarchy["result"]["jobs"]
        .as_object()
        .ok_or_else(|| CoreError::invalid("chain snapshot has no jobs"))?;
    Ok(jobs.iter().find_map(|(job_id, job)| {
        (job["source_id"].as_str() == Some(source_id)).then(|| {
            (
                job_id.clone(),
                job["status"].as_str().unwrap_or("unknown").to_owned(),
            )
        })
    }))
}

fn consecutive_unconsumed_groups(
    lower: &[String],
    consumed: &BTreeSet<String>,
    batch: usize,
    flush: bool,
) -> Vec<Vec<String>> {
    let mut output = Vec::new();
    let mut run = Vec::new();
    let emit = |run: &mut Vec<String>, output: &mut Vec<Vec<String>>, batch: usize, flush: bool| {
        while run.len() >= batch {
            output.push(run.drain(..batch).collect());
        }
        if flush && !run.is_empty() {
            output.push(std::mem::take(run));
        }
    };
    for id in lower {
        if consumed.contains(id) {
            emit(&mut run, &mut output, batch, flush);
        } else {
            run.push(id.clone());
        }
    }
    emit(&mut run, &mut output, batch, flush);
    output
}

fn required_string<'a>(value: &'a Value, key: &str) -> crate::error::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::invalid(format!("scheduler operation response missing {key}")))
}
