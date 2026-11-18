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
    /// Core plans and publishes; the original Harness executes claimed Editors.
    Host,
}

/// §2.4/§3.4 name one scheduling space with three advances, not three
/// mechanisms: adopt in time, wait serially, or let the Worker advance and pay
/// the later rewrite. `wait` never crosses an unpublished job.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePolicy {
    /// Worker resumes only after the current cut's Editor job is published.
    #[default]
    Wait,
    /// Worker may advance once per WB, consuming a host-set interleave permit.
    ///
    /// Current scope: the permit path (per-WB `runtime.budget` receipts) is
    /// what actually releases an early block; this policy value additionally
    /// names an exhaustion as `wait-budget-exhausted` instead of
    /// `wait-editor`. It does not yet select a different block boundary.
    Advance,
    /// Host may choose either per WB, based on the declared wait and estimate.
    ///
    /// Currently identical to `Advance`: there is no per-WB host decision
    /// record yet, so the two names differ only in intent. Keeping the name
    /// reserves the slot without claiming behaviour that is not implemented.
    Adaptive,
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
    #[serde(default = "default_min_level")]
    pub min_level: u32,
    /// Per-level immunity window. A level that has not yet published waits while
    /// it holds at most N replacement candidates; once it holds more, its first
    /// replacement covers every candidate at once. Zero (the default) disables
    /// the window and keeps the eager batch order.
    #[serde(default = "default_protected_prefix_blocks")]
    pub protected_prefix_blocks: usize,
    pub flush: bool,
    pub max_actions: usize,
    pub max_model_calls: usize,
    pub editor_mode: EditorMode,
    pub program_rules: Vec<String>,
}

/// Opt-in immunity window. Hosts that want the draft's quarter-context reserve
/// (two blocks out of every eight) set this explicitly; leaving it unset keeps
/// the pre-existing eager replacement order, so an older policy cannot silently
/// start waiting for blocks it used to replace.
const fn default_min_level()->u32 {1}

pub const fn default_protected_prefix_blocks() -> usize {
    0
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
            min_level: 1,
            protected_prefix_blocks: default_protected_prefix_blocks(),
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

/// A declared wait window the host announced (F30). It is a *declaration*, not
/// a measurement: `seconds` is what the host said, and the scheduler treats it
/// as a prediction that may not come true. Sleep-like declarations never make a
/// tool look started; only `open_calls` on a source does that.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeclaredWait {
    pub wait_id: String,
    pub seconds: u64,
    pub context_id: String,
    pub level: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitDeclaration {
    pub code: String,
    pub required_action: String,
    pub context_id: String,
    pub level: u32,
    pub available: Option<usize>,
    pub required: Option<usize>,
    /// F30: what the scheduler expects to gain by not advancing yet, when the
    /// host declared a wait. Absent means no estimate was available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_wait_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerReport {
    pub context_id: String,
    pub editor_mode: EditorMode,
    pub schedule_policy: SchedulePolicy,
    pub actions: usize,
    pub model_calls_observed: usize,
    pub model_calls_reserved_unknown: usize,
    pub published: Vec<PublishedBlock>,
    #[serde(default)]
    pub host_jobs: Vec<Value>,
    pub interleaves: Vec<InterleaveReceipt>,
    pub needs_flush: bool,
    pub wait_reason: Option<String>,
    pub wait: Option<WaitDeclaration>,
    /// F30: the estimate the scheduler used this pass, in whole
    /// milliseconds, and where it came from. `None` records that no estimate
    /// existed rather than inventing one.
    #[serde(default)]
    pub editor_estimate_ms: Option<u64>,
    #[serde(default)]
    pub editor_estimate_source: Option<String>,
    /// F30: declared waits observed during this pass, so the prediction and
    /// the actual events can be compared instead of merged.
    #[serde(default)]
    pub declared_waits: Vec<DeclaredWait>,
    /// F30/F32/F33: the decision this pass actually made when it stopped,
    /// separate from the reason text.
    #[serde(default)]
    pub decision: Option<String>,
    pub failure: Option<SchedulerFailure>,
    pub completed: bool,
    /// Every Core operation this pass actually issued, in order. A pass that
    /// reports five `chain.get` reads has five reads: the list is evidence,
    /// not a convenience, so a later optimization can be checked against it
    /// instead of assumed.
    #[serde(default)]
    pub operations: Vec<String>,
}

impl SchedulerReport {
    fn new(policy: &SchedulerPolicy) -> Self {
        Self {
            context_id: policy.context_id.clone(),
            editor_mode: policy.editor_mode,
            schedule_policy: SchedulePolicy::Wait,
            actions: 0,
            model_calls_observed: 0,
            model_calls_reserved_unknown: 0,
            published: Vec::new(),
            host_jobs: Vec::new(),
            interleaves: Vec::new(),
            needs_flush: false,
            wait_reason: None,
            wait: None,
            editor_estimate_ms: None,
            editor_estimate_source: None,
            declared_waits: Vec::new(),
            decision: None,
            failure: None,
            completed: false,
            operations: Vec::new(),
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
editor_estimate_seconds: None,
        schedule_policy: SchedulePolicy::Wait,
        chain_interleave_budget: None,
        protected_floor: 0,
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
/// F30: the estimate in force for this pass. It never gates a publish on
    /// its own; it only selects between waiting and advancing.
    editor_estimate_seconds: Option<f64>,
    /// F30/F32/F33: the chain's configured schedule policy, read once per pass
    /// from `config.get`. Defaults to `wait` when the chain has not set one.
    schedule_policy: SchedulePolicy,
    /// F34: the chain-level interleave budget the host configured. The
    /// scheduler hands it to each WB before consuming a permit, so a chain
    /// budget is not silently per-pass.
    chain_interleave_budget: Option<u64>,
    /// Blocks a level must already have published before the scheduler replaces
    /// any of them. Zero means no immunity window.
    protected_floor: usize,
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
        // One configuration read serves both the correction budget and the
        // schedule policy; a second identical `config.get` per pass was pure
        // round-trip cost.
        let config = self.operate("config.get", json!({})).await?;
        if self.policy.editor_mode == EditorMode::Provider {
            if !self.context.allow_model {
                return Err(CoreError::forbidden(
                    "provider scheduling requires host model authorization",
                ));
            }
            self.max_corrections =
                config["result"]["configuration"]["editor_feedback"]["max_corrections"]
                    .as_u64()
                    .unwrap_or(2)
                    .min(3);
        }
        // The pass already holds a full chain snapshot, and `chain.get` reports
        // the same `cut_seq` that `cut.get` reports as `sequence`. A second
        // round-trip for one integer is pure cost.
        self.expected_cut = hierarchy["result"]["cut_seq"]
            .as_u64()
            .ok_or_else(|| CoreError::invalid("cut sequence missing"))?;

self.apply_config(&config)?;
        self.load_declared_waits(&hierarchy)?;
        // Recover pre-existing chains whose higher CBs were published before
        // publication adopted the shared view atomically. The recursive policy
        // bounds the target, and Core's selection fence preserves in-flight
        // snapshots. Fresh publications need no separate selection operation.
        let active = &hierarchy["result"]["role_views"]["worker"];
        if active["context_id"].as_str() == Some(self.policy.context_id.as_str())
            && contexts[&self.policy.context_id]["owner_editor"].is_null()
        {
            let target = contexts[&self.policy.context_id]["levels"].as_object()
                .into_iter().flatten().filter_map(|(level,value)| {
                    let level=level.parse::<u32>().ok()?;
                    (level<=self.policy.max_level && value["cbs"].as_array().is_some_and(|cbs|!cbs.is_empty())).then_some(level)
                }).max().unwrap_or(1);
            if u64::from(target)>active["level"].as_u64().unwrap_or(1) {
                let adopted=self.operate("hierarchy.select",json!({"context_id":self.policy.context_id,
                    "level":target,"roles":["worker","editor"]})).await?;
                self.expected_cut=adopted["result"]["sequence"].as_u64()
                    .ok_or_else(||CoreError::invalid("adopted cut sequence missing"))?;
            }
        }

        // F30: a declaration long enough for the estimated Editor, with
        // pending material waiting, is a real editing opportunity rather than
        // an excuse to idle. Record it before the first block.
        if self.declared_wait_covers_editor() {
            self.report.decision = Some("overlap-declared-wait".into());
        }

        self.protected_floor = self.policy.protected_prefix_blocks;
        if self.policy.min_level<=1 {self.schedule_first_order().await?;}
        if !self.report.host_jobs.is_empty() || self.report.decision.as_deref()==Some("host-volatile-input"){return Ok(());}
        for level in self.policy.min_level.max(2)..=self.policy.max_level {
            self.schedule_higher_level(level).await?;
            if !self.report.host_jobs.is_empty() || self.report.decision.as_deref()==Some("host-volatile-input"){return Ok(());}
        }
        self.report.completed = self.report.failure.is_none() && self.report.wait_reason.is_none();
        Ok(())
    }

/// F30: read the host's declared Editor estimate. Two sources are
    /// accepted, in order: an explicit `config.editor_estimate_ms`, and the
    /// exponentially weighted mean `config.editor_timing.ewma_ms` the host
    /// maintains. Absent means no estimate, and the scheduler records that as
    /// `None` rather than guessing a constant.
    fn apply_config(&mut self, config: &Value) -> crate::error::Result<()> {
        let configuration = &config["result"]["configuration"];
        // F30/F32/F33: the chain decides how Worker and Editor share a
        // schedule. An unknown value is a configuration error, not a default.
        self.schedule_policy = match configuration["schedule_policy"].as_str() {
            None | Some("wait") => SchedulePolicy::Wait,
            Some("advance") => SchedulePolicy::Advance,
            Some("adaptive") => SchedulePolicy::Adaptive,
            Some(other) => {
                return Err(CoreError::invalid(format!(
                    "unknown schedule_policy: {other}"
                )));
            }
        };
        self.report.schedule_policy = self.schedule_policy;
        self.chain_interleave_budget = configuration["interleave_budget"]
            .as_u64()
            .filter(|value| *value > 0);
        if let Some(value) = configuration["editor_estimate_ms"].as_u64() {
            let seconds = value as f64 / 1000.0;
            self.editor_estimate_seconds = Some(seconds);
            self.report.editor_estimate_ms = Some(value);
            self.report.editor_estimate_source = Some("config.editor_estimate_ms".into());
            return Ok(());
        }
        if let Some(value) = configuration["editor_timing"]["ewma_ms"].as_u64() {
            let seconds = value as f64 / 1000.0;
            self.editor_estimate_seconds = Some(seconds);
            self.report.editor_estimate_ms = Some(value);
            self.report.editor_estimate_source = Some("config.editor_timing.ewma_ms".into());
        }
        Ok(())
    }

    /// F30: read what the host declared as waiting. The declaration is kept
    /// apart from the real tool interval; a sleep-like declaration never makes
    /// the tool look started.
    /// Reads the declarations out of the snapshot the pass already holds.
    /// The earlier code issued a second `chain.get` for the same state; a
    /// scheduling pass is the only writer while it runs, so re-reading bought
    /// nothing and cost a full snapshot round-trip.
    fn load_declared_waits(
        &mut self, hierarchy: &Value,
    ) -> crate::error::Result<()> {
        // `chain.get` carries the persistent declarations the host registered
        // through `runtime.wait`. Only open declarations count; a completed or
        // cancelled wait no longer predicts anything.
        let declarations = hierarchy["result"]["declared_waits"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut waits = Vec::new();
        for (wait_id, entry) in declarations {
            if entry["status"].as_str() != Some("declared") {
                continue;
            }
            waits.push(DeclaredWait {
                seconds: entry["seconds"].as_u64().unwrap_or(0),
                context_id: self.policy.context_id.clone(),
                level: entry["level"].as_u64().unwrap_or(1) as u32,
                wait_id,
            });
        }
        waits.sort();
        self.report.declared_waits = waits;
        Ok(())
    }

    /// F30: whether a declared wait is long enough that the Editor can
    /// plausibly finish inside it. With no estimate this returns `false` and
    /// the scheduler falls back to waiting rather than inventing a guess.
    fn declared_wait_covers_editor(&self) -> bool {
        let Some(estimate) = self.editor_estimate_seconds else {
            return false;
        };
        if estimate <= 0.0 {
            return false;
        }
        self.report
            .declared_waits
            .iter()
            .any(|wait| wait.seconds as f64 >= estimate)
    }

    /// F30: stop the pass and record the decision next to the reason.
    fn halt(&mut self, decision: &str, wait: WaitDeclaration, reason: String) {
        self.report.decision = Some(decision.into());
        self.report.wait_reason = Some(reason);
        self.report.wait = Some(wait);
    }

    /// The immunity window is per level: while a level has published nothing and
    /// holds no more than `protected_prefix_blocks` replacement candidates, the
    /// scheduler waits instead of replacing. Once the level holds more than that,
    /// its first replacement covers every accumulated block at once, so the
    /// active prefix a Worker reads afterwards is never re-read block by block.
    ///
    /// `flush` is the host escape hatch: an explicit flush publishes immediately
    /// and ignores the window.
    fn window_open(
        &mut self,
        hierarchy: &Value,
        level: u32,
        available: usize,
    ) -> crate::error::Result<bool> {
        if self.protected_floor == 0 || self.policy.flush {
            return Ok(false);
        }
        if published_cbs(hierarchy, &self.policy.context_id, level)? > 0 {
            return Ok(false);
        }
        if available <= self.protected_floor {
            let required = self.protected_floor + 1;
            self.report.wait_reason.get_or_insert_with(|| {
                format!(
                    "level {level} holds {available} block(s) inside the protected prefix; \
                     the first replacement waits for {required} blocks"
                )
            });
            self.report.wait.get_or_insert(WaitDeclaration {
                code: "protected_prefix".into(),
                required_action: "host may flush to replace the prefix immediately".into(),
                context_id: self.policy.context_id.clone(),
                level,
                available: Some(available),
                required: Some(required),
                expected_wait_seconds: None,
            });
            return Ok(true);
        }
        Ok(false)
    }

    /// True when this pass is the level's first replacement and must therefore
    /// cover the whole accumulated prefix rather than one batch.
    fn catch_up(&self, hierarchy: &Value, level: u32) -> crate::error::Result<bool> {
        Ok(self.protected_floor > 0
            && published_cbs(hierarchy, &self.policy.context_id, level)? == 0)
    }

    async fn schedule_first_order(&mut self) -> crate::error::Result<()> {
        loop {
            let hierarchy = self.operate("chain.get", json!({})).await?;
            let pending = pending_sources(&hierarchy, &self.policy.context_id, 1)?;
            if pending.is_empty() {
                return Ok(());
            }
            if self.window_open(&hierarchy, 1, pending.len())? {
                return Ok(());
            }
            let catch_up = self.catch_up(&hierarchy, 1)?;

            let open_wbs = self.timeline_rows(1, "wb").await?;
            let existing = open_wbs.iter().find(|row| {
                row["source_id"].as_str() == Some(&pending[0]) && row["cb_id"].is_null()
            });

            let source_id = if let Some(wb) = existing {
                wb["source_id"].as_str().unwrap().to_owned()
            } else {
                let mut selected = Vec::new();
                let mut units = 0_usize;
                // The first replacement after the protected prefix covers every
                // accumulated block in one CB; later replacements use batches.
                let take = if catch_up {
                    pending.len()
                } else {
                    self.policy.max_sources_per_wb
                };
                for source_id in pending.iter().take(take) {
                    let read = self
                        .operate("source.read", json!({"source_id":source_id,"group":true}))
                        .await?;
                    let source = &read["result"]["source"];
                    if source["open_calls"]
                        .as_array()
                        .is_some_and(|calls| !calls.is_empty())
                    {
                        // F30/F31: a tool whose result the host has not
                        // closed yet is real work in progress, not a
                        // prediction. Editing may start, but the Worker
                        // cannot cross an unpublished block.
                        self.halt(
                            "wait-unresolved-tool",
                            WaitDeclaration {
                                code: "unresolved_tool_calls".into(),
                                required_action: "host must close or reject the open native call"
                                    .into(),
                                context_id: self.policy.context_id.clone(),
                                level: 1,
                                available: Some(pending.len()),
                                required: None,
                                expected_wait_seconds: None,
                            },
                            "pending source has unresolved native tool calls".into(),
                        );
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
                    // F30/F34: a declared wait long enough for the estimated
                    // Editor is a real opportunity, so the block is formed now
                    // instead of idling to the minimum batch. That path still
                    // consumes no interleave permit: it adopts in time, it does
                    // not overtake.
                    if self.declared_wait_covers_editor() {
                        self.report.decision = Some("overlap-declared-wait".into());
                    } else {
                    // F34: an early block is allowed only with a host-set,
                    // per-WB permit; the receipt is idempotent and charged
                    // once. Without budget the pass waits as before.
                    let wb_source_id = selected[0].clone();
                    let permit_id = format!("early:{wb_source_id}");
                    // F34: make sure this WB has a budget. A chain-level
                    // budget is applied once per WB; the ledger stays per-WB
                    // so a permit is charged to the block that spent it.
                    if let Some(budget) = self.chain_interleave_budget {
                        // A WB that already spent its budget keeps its ledger;
                        // re-applying cannot raise the remaining permits, and
                        // the consumption below is what reports exhaustion.
                        let set = self
                            .operate(
                                "runtime.budget",
                                json!({"wb_source_id":wb_source_id,"budget":budget}),
                            )
                            .await?;
                        if set["result"]["budget"].as_u64().unwrap_or(0) < budget {
                            return Err(CoreError::conflict(
                                "per-WB interleave budget is below the chain budget",
                            ));
                        }
                    }
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
                            let code = error.code.to_string();
                            // F30/F34: the budget is exhausted, so the
                            // declared advance is refused. Whether the pass
                            // then waits or stops is recorded as a decision
                            // instead of a free-form string.
                            let decision = match self.schedule_policy {
                                SchedulePolicy::Advance | SchedulePolicy::Adaptive => {
                                    "wait-budget-exhausted"
                                }
                                SchedulePolicy::Wait => "wait-editor",
                            };
                            self.report.needs_flush = true;
                            self.halt(
                                decision,
                                WaitDeclaration {
                                    code: format!("interleave_{code}"),
                                    required_action: "host must grant a per-WB budget or explicitly flush"
                                        .into(),
                                    context_id: self.policy.context_id.clone(),
                                    level: 1,
                                    available: Some(selected.len()),
                                    required: Some(self.policy.min_sources_per_wb),
                                    expected_wait_seconds: self
                                        .report
                                        .editor_estimate_ms
                                        .map(|value| (value + 999) / 1000),
                                },
                                format!(
                                    "pending batch is below min_sources_per_wb; explicit flush required (interleave {code})"
                                ),
                            );
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
            if !self.report.host_jobs.is_empty() || self.report.decision.as_deref()==Some("host-volatile-input"){return Ok(());}
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
        let higher_rows:Vec<Value>=higher_rows.into_iter().filter(|row|row["superseded_by"].is_null()).collect();
        let mut volatile_ids=Vec::<String>::new();
        for pending in levels.get(&level_key).and_then(|row|row["pending"].as_array()).into_iter().flatten() {
            let id=pending.as_str().ok_or_else(||CoreError::invalid("pending source id missing"))?;
            if higher_rows.iter().any(|row|row["source_id"]==id&&row["reason"]!="volatile-continuation"){break;}
            let source=self.operate("source.read",json!({"source_id":id})).await?;
            if source["result"]["source"]["origin"]["kind"]!="volatile-continuation"{break;}
            volatile_ids.push(id.to_owned());
        }
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
        for row in higher_rows.iter().filter(|row| row["cb_id"].is_null() && !volatile_ids.iter().any(|id|row["source_id"]==*id)) {
            let source = required_string(row, "source_id")?;
            let hierarchy = self.operate("chain.get", json!({})).await?;
            self.ensure_no_ambiguous_prior_job(&hierarchy, source)?;
            self.edit_and_publish(level, source).await?;
            if !self.report.host_jobs.is_empty() || self.report.decision.as_deref()==Some("host-volatile-input"){return Ok(());}
        }

        let available_members = lower_ids
            .iter()
            .filter(|id| !consumed.contains(*id))
            .count();
        if self.window_open(&snapshot, level, available_members)? {
            return Ok(());
        }
        // The level's first replacement swallows the whole accumulated prefix in
        // one ordered higher WB; later replacements keep the configured batch.
        let catch_up = self.catch_up(&snapshot, level)?;
        let batch = if catch_up {
            available_members.max(1)
        } else {
            batch
        };
        let mut groups = consecutive_unconsumed_groups(&lower_ids, &consumed, batch, self.policy.flush);
        if groups.is_empty()&&!volatile_ids.is_empty()&&self.policy.flush{groups.push(Vec::new());}
        if groups.is_empty()&&!volatile_ids.is_empty(){
            self.halt("host-volatile-input",WaitDeclaration{code:"higher_volatile_input".into(),required_action:"continue Worker until a fresh lower CB interval is available, or explicitly flush the pending V".into(),context_id:self.policy.context_id.clone(),level,available:Some(available_members),required:Some(batch),expected_wait_seconds:None},"same-level V preserved while Worker may generate the next lower material".into());
            return Ok(());
        }
        let scheduled_members: usize = groups.iter().map(Vec::len).sum();
        if !self.policy.flush && scheduled_members < available_members {
            self.report.needs_flush = true;
            self.report.decision.get_or_insert_with(|| "wait-level-batch".into());
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
                expected_wait_seconds: None,
            });
        }
        for members in groups {
            let formed = self
                .operate(
                    "hierarchy.form",
                    json!({
                        "context_id":self.policy.context_id,"level":level,"cb_ids":members,"volatile_source_ids":volatile_ids
                    }),
                )
                .await?;
            volatile_ids.clear();
            let source = required_string(&formed["result"], "source_id")?.to_owned();
            self.edit_and_publish(level, &source).await?;
            if !self.report.host_jobs.is_empty() || self.report.decision.as_deref()==Some("host-volatile-input"){return Ok(());}
        }
        Ok(())
    }

    async fn edit_and_publish(&mut self, level: u32, source_id: &str) -> crate::error::Result<()> {
        let hierarchy = self.operate("chain.get", json!({})).await?;
        let prior = find_job_for_source(&hierarchy, source_id)?;
        let (job_id, status, new_candidate) = if let Some((job_id, status)) = prior {
            if self.policy.editor_mode!=EditorMode::Host && (status == "failed" || status == "inflight" || status == "superseded") {
                return Err(CoreError::conflict(
                    "prior Editor job is failed or ambiguous; scheduler will not retry it",
                ));
            }
            (job_id, status, false)
        } else {
            let operation = match self.policy.editor_mode {
                EditorMode::Provider => "edit.prepare",
                EditorMode::ProgramFixture => "edit.program",
                EditorMode::Host => "edit.host.prepare",
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

        if self.policy.editor_mode==EditorMode::Host {
            let view=self.operate("edit.host.get",json!({"job_id":job_id})).await?["result"].clone();
            if view["status"]!="ready" {
                self.report.host_jobs.push(view);
                self.halt("host-editor",WaitDeclaration{code:"host_editor".into(),required_action:"host must execute a newly claimed Editor exactly once, submit its delivery, then resume this Rust scheduler".into(),context_id:self.policy.context_id.clone(),level,available:None,required:None,expected_wait_seconds:None},"durable host Editor job requires execution or explicit unknown recovery".into());
                return Ok(());
            }
        }
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
            // F32/F33: the job exists and is still running. The decision is
            // "adopt in time" when the estimate says the Editor finishes
            // inside the declared wait, otherwise "wait-editor" so the cost
            // of a second Editor pass stays visible.
            let decision = if self.declared_wait_covers_editor() {
                "adopt-in-declared-wait"
            } else {
                "wait-editor"
            };
            self.halt(
                decision,
                WaitDeclaration {
                    code: "editor_not_ready".into(),
                    required_action: "host must wait for the in-flight Editor or abandon it"
                        .into(),
                    context_id: self.policy.context_id.clone(),
                    level,
                    available: None,
                    required: None,
                    expected_wait_seconds: self
                        .report
                        .editor_estimate_ms
                        .map(|value| (value + 999) / 1000),
                },
                "existing Editor job is not ready for publication".into(),
            );
            return Ok(());
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
        if self.policy.editor_mode==EditorMode::Host && level>=2 && publish["result"]["volatile_source_id"].is_string() {
            self.halt("host-volatile-input",WaitDeclaration{code:"higher_volatile_input".into(),required_action:"continue Worker for new lower material or explicitly flush the pending V".into(),context_id:self.policy.context_id.clone(),level,available:None,required:None,expected_wait_seconds:None},"new same-level V is preserved for a subsequent editing boundary".into());
        }
        Ok(())
    }

    fn ensure_no_ambiguous_prior_job(
        &self,
        hierarchy: &Value,
        source_id: &str,
    ) -> crate::error::Result<()> {
        if self.policy.editor_mode==EditorMode::Host {return Ok(());}
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
        self.report.operations.push(operation.to_owned());
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
        || policy.min_level == 0
        || policy.min_level > policy.max_level
        || policy.max_level == 0
        || policy.max_level > 128
        || policy.max_actions == 0
        || policy.protected_prefix_blocks > 64
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

/// Number of published CBs at one level, from a `chain.get` snapshot.
fn published_cbs(hierarchy: &Value, context_id: &str, level: u32) -> crate::error::Result<usize> {
    let key = level.to_string();
    Ok(
        hierarchy["result"]["contexts"][context_id]["levels"][key.as_str()]["cbs"]
            .as_array()
            .map_or(0, Vec::len),
    )
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
