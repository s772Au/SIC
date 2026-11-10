//! Explicitly enabled, single-task poller for trusted peer work.
//!
//! Constructing a watcher does not spawn it. `cycle` is a one-shot check and
//! `watch_loop` must be called explicitly by the host. Durable task receipts
//! and process intents remain authoritative across watcher restarts.

use crate::{
    peer_process,
    peer_runner_client::{HostPermissions, PeerRunnerClient},
    peer_supervisor::{self, SupervisorConfig},
    peer_tasks,
    storage::Store,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc, time::Duration};

const WATCH_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS sic_peer_watch_attempts (
    namespace TEXT NOT NULL,
    task_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('intent','blocked','reply_delivered','reply_present')),
    reason TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(namespace,task_id,revision)
)";
const MAX_PAGE_SIZE: usize = 100;
const MAX_SCAN_PAGES: usize = 32;
const MAX_REPORTED_IN_MEMORY: usize = 4096;

/// Trusted host policy. `enabled` intentionally defaults to false; the host
/// must opt in and explicitly call `watch_loop` to create a background task.
pub struct WatchConfig {
    pub enabled: bool,
    pub namespace: String,
    pub poll_interval: Duration,
    pub max_tasks_per_cycle: usize,
    pub page_size: usize,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            namespace: "disabled".into(),
            poll_interval: Duration::from_secs(5),
            max_tasks_per_cycle: 1,
            page_size: 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchError {
    InvalidConfiguration,
    CoreUnavailable,
    InvalidCoreResponse,
    StorageUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlIds {
    pub claim_id: String,
    pub prepare_id: String,
    pub launch_id: String,
    pub reply_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleOutcome {
    Disabled,
    Idle,
    Replied {
        task_id: String,
        reply_message_id: String,
    },
    ReplyAlreadyPresent {
        task_id: String,
    },
    Blocked {
        task_id: String,
        revision: u64,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReceipt {
    pub state: &'static str,
    pub task_id: String,
    pub peer_chain_id: String,
    pub reply_message_id: String,
    pub replayed: bool,
    pub requester_adopted_cb: bool,
}

pub type RunnerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RunReceipt, &'static str>> + Send + 'a>>;

/// Injection point for the trusted one-shot supervisor; tests use synthetic
/// outcomes and never start DSH or call a model.
pub trait PeerTaskRunner: Send + Sync {
    fn run<'a>(&'a self, config: &'a SupervisorConfig, bearer: &'a str) -> RunnerFuture<'a>;
}

struct ProcessSupervisor;

impl PeerTaskRunner for ProcessSupervisor {
    fn run<'a>(&'a self, config: &'a SupervisorConfig, bearer: &'a str) -> RunnerFuture<'a> {
        Box::pin(async move {
            let outcome = peer_supervisor::run(config, bearer.to_owned())
                .await
                .map_err(|_| "supervisor_outcome_unknown")?;
            Ok(RunReceipt {
                state: outcome.state,
                task_id: outcome.task_id,
                peer_chain_id: outcome.peer_chain_id,
                reply_message_id: outcome.reply_message_id,
                replayed: outcome.replayed,
                requester_adopted_cb: outcome.requester_adopted_cb,
            })
        })
    }
}

/// Stable, domain-separated control identities. These contain no task text.
pub fn deterministic_control_ids(namespace: &str, task_id: &str) -> Result<ControlIds, WatchError> {
    if !valid_identity(namespace, 128) || !valid_identity(task_id, 128) {
        return Err(WatchError::InvalidConfiguration);
    }
    let id = |kind: &str| -> Result<String, WatchError> {
        let digest = crate::storage::digest(&json!({
            "namespace":namespace,
            "task_id":task_id,
            "purpose":kind
        }))
        .map_err(|_| WatchError::InvalidConfiguration)?;
        Ok(format!("peer_watch_{kind}_{digest}"))
    };
    Ok(ControlIds {
        claim_id: id("claim")?,
        prepare_id: id("prepare")?,
        launch_id: id("launch")?,
        reply_id: id("reply")?,
    })
}

fn valid_identity(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub struct PeerWatch {
    config: WatchConfig,
    template: SupervisorConfig,
    bearer: String,
    client: PeerRunnerClient,
    store: Store,
    runner: Arc<dyn PeerTaskRunner>,
    cursor: u64,
    reported: HashSet<(String, u64)>,
}

impl PeerWatch {
    pub async fn new(
        config: WatchConfig,
        template: SupervisorConfig,
        bearer: impl Into<String>,
    ) -> Result<Self, WatchError> {
        Self::with_runner(config, template, bearer, Arc::new(ProcessSupervisor)).await
    }

    pub async fn with_runner(
        config: WatchConfig,
        template: SupervisorConfig,
        bearer: impl Into<String>,
        runner: Arc<dyn PeerTaskRunner>,
    ) -> Result<Self, WatchError> {
        if !valid_identity(&config.namespace, 128)
            || config.poll_interval.is_zero()
            || config.poll_interval < Duration::from_millis(500)
            || config.poll_interval > Duration::from_secs(3600)
            || config.max_tasks_per_cycle != 1
            || config.page_size == 0
            || config.page_size > MAX_PAGE_SIZE
            || !valid_identity(&template.actor, 128)
            || !valid_identity(&template.requester_chain_id, 128)
            || !valid_identity(&template.reply_socket_id, 128)
            || template.api_timeout_seconds == 0
            || template.api_timeout_seconds > 3600
        {
            return Err(WatchError::InvalidConfiguration);
        }
        peer_supervisor::validate(&template).map_err(|_| WatchError::InvalidConfiguration)?;
        let bearer = bearer.into();
        let client = PeerRunnerClient::new_with_timeout(
            &template.core_base_url,
            bearer.clone(),
            &template.actor,
            HostPermissions::new(true, true, false),
            Duration::from_secs(template.api_timeout_seconds),
        )
        .map_err(|_| WatchError::InvalidConfiguration)?;
        let store = Store::open(&template.db_path)
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        {
            let mut tx = store
                .begin()
                .await
                .map_err(|_| WatchError::StorageUnavailable)?;
            peer_tasks::initialize_in_transaction(&mut tx)
                .await
                .map_err(|_| WatchError::StorageUnavailable)?;
            sqlx::query(WATCH_SCHEMA)
                .execute(&mut *tx)
                .await
                .map_err(|_| WatchError::StorageUnavailable)?;
            tx.commit()
                .await
                .map_err(|_| WatchError::StorageUnavailable)?;
        }
        Ok(Self {
            config,
            template,
            bearer,
            client,
            store,
            runner,
            cursor: 0,
            reported: HashSet::new(),
        })
    }

    /// Inspect at most one actionable task. It does no work while disabled.
    pub async fn cycle(&mut self) -> Result<CycleOutcome, WatchError> {
        if !self.config.enabled {
            return Ok(CycleOutcome::Disabled);
        }
        let page_size = self.config.page_size;
        let mut offset = self.cursor;
        for _ in 0..MAX_SCAN_PAGES {
            let page = self
                .client
                .operate(
                    "peer.tasks",
                    Some(&self.template.requester_chain_id),
                    &json!({"offset":offset,"limit":page_size}),
                )
                .await
                .map_err(|_| WatchError::CoreUnavailable)?;
            let tasks = page["result"]["tasks"]
                .as_array()
                .ok_or(WatchError::InvalidCoreResponse)?;
            if tasks.is_empty() {
                self.cursor = 0;
                break;
            }
            let page_len = tasks.len();
            for (index, task) in tasks.iter().enumerate() {
                let observed_offset = offset.saturating_add(index as u64);
                self.cursor = observed_offset.saturating_add(1);
                if !self.belongs_to_requester(task) {
                    continue;
                }
                let task_id = match task["task_id"].as_str() {
                    Some(value) if valid_identity(value, 128) => value.to_owned(),
                    _ => continue,
                };
                if !task["peer_chain_id"]
                    .as_str()
                    .is_some_and(|value| valid_identity(value, 128))
                {
                    continue;
                }
                let revision = task["revision"]
                    .as_u64()
                    .ok_or(WatchError::InvalidCoreResponse)?;
                if revision > i64::MAX as u64 {
                    return Err(WatchError::InvalidCoreResponse);
                }
                let ids = deterministic_control_ids(&self.config.namespace, &task_id)?;
                let state = task["state"].as_str().unwrap_or_default();
                let eligible_pending = state == "pending" && task["claim_id"].is_null();
                let eligible_owned = matches!(state, "claimed" | "prepared")
                    && task["claim_id"].as_str() == Some(ids.claim_id.as_str())
                    && task["claimed_by"].as_str() == Some(self.template.actor.as_str());
                if !eligible_pending && !eligible_owned {
                    continue;
                }
                // Pending work that already produced an ambiguous runner result
                // remains parked in this process. Claimed work is different:
                // its durable process receipt may change from `running` to
                // `exited`, so inspect it on every cycle without re-launching.
                if eligible_pending && self.reported.contains(&(task_id.clone(), revision)) {
                    continue;
                }
                match self
                    .process_candidate(task, &task_id, revision, &ids, eligible_pending)
                    .await?
                {
                    CandidateResult::Done(outcome) => return Ok(outcome),
                    CandidateResult::Blocked(outcome) => return Ok(outcome),
                    CandidateResult::Ignore => {}
                }
            }
            if page_len < page_size {
                self.cursor = 0;
                break;
            }
            offset = offset.saturating_add(page_len as u64);
            self.cursor = offset;
        }
        Ok(CycleOutcome::Idle)
    }

    /// Explicit cancellable background loop. Constructing `PeerWatch` never
    /// starts it; errors terminate the loop instead of retrying automatically.
    pub async fn watch_loop(
        &mut self,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), WatchError> {
        if !self.config.enabled || *stop.borrow() {
            return Ok(());
        }
        loop {
            self.cycle().await?;
            tokio::select! {
                _ = tokio::time::sleep(self.config.poll_interval) => {},
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn belongs_to_requester(&self, task: &Value) -> bool {
        task["actor"] == self.template.actor
            && task["requester_chain_id"] == self.template.requester_chain_id
    }

    async fn process_candidate(
        &mut self,
        task: &Value,
        task_id: &str,
        revision: u64,
        ids: &ControlIds,
        is_pending: bool,
    ) -> Result<CandidateResult, WatchError> {
        if matches!(
            self.watch_state(task_id, revision).await?.as_deref(),
            Some("reply_delivered" | "reply_present")
        ) {
            self.note_reported(task_id, revision);
            return Ok(CandidateResult::Ignore);
        }
        self.set_template_task(task, task_id, ids);
        let mut reply_tx = self
            .store
            .begin()
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        let reply_exists =
            sqlx::query("SELECT 1 FROM sic_peer_task_replies WHERE task_id=? AND reply_id=?")
                .bind(task_id)
                .bind(&self.template.reply_id)
                .fetch_optional(&mut *reply_tx)
                .await
                .map_err(|_| WatchError::StorageUnavailable)?
                .is_some();
        reply_tx
            .commit()
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        if reply_exists {
            self.save_watch_state(task_id, revision, "reply_present", None)
                .await?;
            self.note_reported(task_id, revision);
            return Ok(CandidateResult::Done(CycleOutcome::ReplyAlreadyPresent {
                task_id: task_id.to_owned(),
            }));
        }

        if !is_pending {
            match peer_process::inspect_launch(
                &self.store,
                &self.template.actor,
                &self.template.peer_chain_id,
                &self.template.task_id,
                &self.template.claim_id,
                &self.template.launch_id,
            )
            .await
            {
                Ok(Some(receipt)) if receipt.state == "exited" && receipt.exit_code == Some(0) => {}
                Ok(Some(_)) => {
                    return Ok(self
                        .block(task_id, revision, "process_outcome_unknown")
                        .await?);
                }
                Ok(None) => {
                    return Ok(self
                        .block(task_id, revision, "claim_has_no_process_intent")
                        .await?);
                }
                Err(_) => return Err(WatchError::StorageUnavailable),
            }
        } else if self.watch_state(task_id, revision).await?.is_some() {
            // A prior local intent is not enough to assume a dispatch did not
            // occur. Core's pending receipt makes this request safe; the same
            // task/claim IDs still provide the authoritative CAS.
        }

        self.save_watch_state(task_id, revision, "intent", None)
            .await?;
        self.note_reported(task_id, revision);
        let result = self.runner.run(&self.template, &self.bearer).await;
        match result {
            Ok(receipt)
                if receipt.state == "reply_delivered"
                    && receipt.task_id == task_id
                    && receipt.peer_chain_id == self.template.peer_chain_id
                    && !receipt.reply_message_id.trim().is_empty()
                    && !receipt.requester_adopted_cb =>
            {
                self.save_watch_state(task_id, revision, "reply_delivered", None)
                    .await?;
                Ok(CandidateResult::Done(CycleOutcome::Replied {
                    task_id: task_id.to_owned(),
                    reply_message_id: receipt.reply_message_id,
                }))
            }
            _ => {
                self.save_watch_state(
                    task_id,
                    revision,
                    "blocked",
                    Some("supervisor_outcome_unknown"),
                )
                .await?;
                Ok(CandidateResult::Blocked(CycleOutcome::Blocked {
                    task_id: task_id.to_owned(),
                    revision,
                    reason: "supervisor_outcome_unknown",
                }))
            }
        }
    }

    fn set_template_task(&mut self, task: &Value, task_id: &str, ids: &ControlIds) {
        self.template.task_id = task_id.to_owned();
        self.template.peer_chain_id = task["peer_chain_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.template.claim_id = ids.claim_id.clone();
        self.template.prepare_id = ids.prepare_id.clone();
        self.template.launch_id = ids.launch_id.clone();
        self.template.reply_id = ids.reply_id.clone();
        // reply_socket_id, actor and requester route are never sourced from a task.
    }

    async fn block(
        &mut self,
        task_id: &str,
        revision: u64,
        reason: &'static str,
    ) -> Result<CandidateResult, WatchError> {
        self.save_watch_state(task_id, revision, "blocked", Some(reason))
            .await?;
        self.note_reported(task_id, revision);
        Ok(CandidateResult::Blocked(CycleOutcome::Blocked {
            task_id: task_id.to_owned(),
            revision,
            reason,
        }))
    }

    fn note_reported(&mut self, task_id: &str, revision: u64) {
        if self.reported.len() >= MAX_REPORTED_IN_MEMORY {
            // Durable watcher receipts and authoritative Core state remain
            // the cross-cycle/cross-restart guard when the fast set is pruned.
            self.reported.clear();
        }
        self.reported.insert((task_id.to_owned(), revision));
    }

    async fn watch_state(
        &self,
        task_id: &str,
        revision: u64,
    ) -> Result<Option<String>, WatchError> {
        let revision = i64::try_from(revision).map_err(|_| WatchError::InvalidCoreResponse)?;
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        let row = sqlx::query("SELECT state FROM sic_peer_watch_attempts WHERE namespace=? AND task_id=? AND revision=?")
            .bind(&self.config.namespace).bind(task_id).bind(revision)
            .fetch_optional(&mut *tx).await.map_err(|_| WatchError::StorageUnavailable)?;
        let state = row
            .map(|row| row.try_get::<String, _>("state"))
            .transpose()
            .map_err(|_| WatchError::StorageUnavailable)?;
        tx.commit()
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        Ok(state)
    }

    async fn save_watch_state(
        &self,
        task_id: &str,
        revision: u64,
        state: &str,
        reason: Option<&str>,
    ) -> Result<(), WatchError> {
        let revision = i64::try_from(revision).map_err(|_| WatchError::InvalidCoreResponse)?;
        let mut tx = self
            .store
            .begin()
            .await
            .map_err(|_| WatchError::StorageUnavailable)?;
        sqlx::query("INSERT INTO sic_peer_watch_attempts(namespace,task_id,revision,state,reason) VALUES(?,?,?,?,?) ON CONFLICT(namespace,task_id,revision) DO UPDATE SET state=excluded.state,reason=excluded.reason,updated_at=CURRENT_TIMESTAMP WHERE sic_peer_watch_attempts.state IS NOT excluded.state OR sic_peer_watch_attempts.reason IS NOT excluded.reason")
            .bind(&self.config.namespace).bind(task_id).bind(revision).bind(state).bind(reason)
            .execute(&mut *tx).await.map_err(|_| WatchError::StorageUnavailable)?;
        tx.commit()
            .await
            .map_err(|_| WatchError::StorageUnavailable)
    }
}

enum CandidateResult {
    Done(CycleOutcome),
    Blocked(CycleOutcome),
    Ignore,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use serde_json::json;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct ApiState {
        tasks: Arc<Mutex<Vec<Value>>>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    async fn api(State(state): State<ApiState>, Json(request): Json<Value>) -> Json<Value> {
        state.requests.lock().await.push(request.clone());
        let offset = request["payload"]["offset"].as_u64().unwrap_or(0) as usize;
        let limit = request["payload"]["limit"].as_u64().unwrap_or(100) as usize;
        let tasks = state.tasks.lock().await;
        Json(json!({"ok":true,"operation":"peer.tasks","result":{
            "tasks":tasks.iter().skip(offset).take(limit).cloned().collect::<Vec<_>>(),
            "offset":offset,"limit":limit
        }}))
    }

    struct MockRunner {
        calls: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<(String, String, String, String, String)>>>,
        fail: bool,
    }

    impl PeerTaskRunner for MockRunner {
        fn run<'a>(&'a self, config: &'a SupervisorConfig, _bearer: &'a str) -> RunnerFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.seen.lock().await.push((
                    config.task_id.clone(),
                    config.peer_chain_id.clone(),
                    config.claim_id.clone(),
                    config.prepare_id.clone(),
                    config.reply_socket_id.clone(),
                ));
                if self.fail {
                    return Err("supervisor_outcome_unknown");
                }
                Ok(RunReceipt {
                    state: "reply_delivered",
                    task_id: config.task_id.clone(),
                    peer_chain_id: config.peer_chain_id.clone(),
                    reply_message_id: "reply-message-fixture".into(),
                    replayed: false,
                    requester_adopted_cb: false,
                })
            })
        }
    }

    fn config(dir: &Path) -> SupervisorConfig {
        let db = dir.join("sic.sqlite");
        if !db.exists() {
            std::fs::write(&db, b"will be replaced by Store::create").unwrap();
        }
        let cli = dir.join("cli.js");
        if !cli.exists() {
            std::fs::write(&cli, b"// test fixture only").unwrap();
        }
        let executable = std::env::current_exe().unwrap();
        serde_json::from_value(json!({
            "db_path":db,"core_base_url":"http://127.0.0.1:1234",
            "bridge_base_url":"http://127.0.0.1:1234/v1","bearer_env":"SIC_CORE_TOKEN",
            "actor":"watcher-owner","requester_chain_id":"requester-route","peer_chain_id":"template-peer",
            "task_id":"template-task","reply_socket_id":"trusted-reply-socket",
            "claim_id":"template-claim","prepare_id":"template-prepare","launch_id":"template-launch","reply_id":"template-reply",
            "policy":{"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":1,"max_units_per_wb":64,
                "level_batch_sizes":{},"max_level":1,"flush":true,"max_actions":32,"max_model_calls":4,
                "editor_mode":"provider","program_rules":[]},
            "dsh_executable":executable,"dsh_cli_js":cli,"dsh_home":dir,
            "dsh_profile":"fixture-profile","working_directory":dir,"state_directory":dir,
            "model_alias":"sic-worker","max_tokens":4096,"timeout_seconds":30,"api_timeout_seconds":30,
            "prices":{"input":0,"output":0,"cache_read":0,"cache_write":0},"allowed_tools":["fixture"],
            "prompt":"fixture prompt"
        })).unwrap()
    }

    async fn create_store(path: &Path, tasks: &[Value]) -> Store {
        let store = Store::create(path).await.unwrap();
        let mut tx = store.begin().await.unwrap();
        peer_tasks::initialize_in_transaction(&mut tx)
            .await
            .unwrap();
        for chain in ["requester-route", "peer-a", "peer-b"] {
            let state = json!({"chain_id":chain,"owners":["watcher-owner"]}).to_string();
            sqlx::query("INSERT INTO chains(id,state) VALUES(?,?)")
                .bind(chain)
                .bind(state)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        for task in tasks {
            let claim = task["claim_id"].as_str();
            sqlx::query("INSERT INTO sic_peer_tasks(task_id,actor,requester_chain_id,peer_chain_id,socket_id,checkpoint_id,request_id,message_id,lifecycle,state,claim_id,claimed_by,revision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(task["task_id"].as_str().unwrap()).bind(task["actor"].as_str().unwrap())
                .bind(task["requester_chain_id"].as_str().unwrap()).bind(task["peer_chain_id"].as_str().unwrap())
                .bind("spawn-socket").bind("checkpoint").bind(task["request_id"].as_str().unwrap())
                .bind(task["message_id"].as_str().unwrap()).bind("P").bind(task["state"].as_str().unwrap())
                .bind(claim).bind(task["claimed_by"].as_str()).bind(task["revision"].as_u64().unwrap() as i64)
                .execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        store
    }

    fn task(
        task_id: &str,
        peer: &str,
        state: &str,
        revision: u64,
        claim_id: Option<&str>,
    ) -> Value {
        json!({"task_id":task_id,"actor":"watcher-owner","requester_chain_id":"requester-route",
            "peer_chain_id":peer,"state":state,"revision":revision,"claim_id":claim_id,
            "claimed_by":if claim_id.is_some(){json!("watcher-owner")}else{Value::Null},
            "request_id":format!("request-{task_id}"),"message_id":format!("message-{task_id}")})
    }

    async fn start_api(tasks: Vec<Value>) -> (String, ApiState, tokio::task::JoinHandle<()>) {
        let state = ApiState {
            tasks: Arc::new(Mutex::new(tasks)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let serve_state = state.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/api/v2/operations", post(api))
                    .with_state(serve_state),
            )
            .await
            .unwrap();
        });
        (base, state, server)
    }

    async fn watcher(
        dir: &Path,
        tasks: Vec<Value>,
        page_size: usize,
        runner: Arc<dyn PeerTaskRunner>,
    ) -> (PeerWatch, ApiState, tokio::task::JoinHandle<()>) {
        let db_path = dir.join("sic.sqlite");
        if !db_path.exists() {
            let _ = create_store(&db_path, &tasks).await;
        }
        let (base, state, server) = start_api(tasks).await;
        let mut template = config(dir);
        template.db_path = db_path;
        template.core_base_url = base;
        let watch_config = WatchConfig {
            enabled: true,
            namespace: "fixture-watch".into(),
            poll_interval: Duration::from_millis(500),
            max_tasks_per_cycle: 1,
            page_size,
        };
        let watch = PeerWatch::with_runner(watch_config, template, "mock-bearer", runner)
            .await
            .unwrap();
        (watch, state, server)
    }

    #[test]
    fn control_ids_are_stable_separated_and_do_not_contain_task_text() {
        let first = deterministic_control_ids("ops-A", "task-123").unwrap();
        assert_eq!(
            first,
            deterministic_control_ids("ops-A", "task-123").unwrap()
        );
        assert_ne!(
            first,
            deterministic_control_ids("ops-B", "task-123").unwrap()
        );
        assert_ne!(first.claim_id, first.prepare_id);
        assert_ne!(first.launch_id, first.reply_id);
        assert!(!first.claim_id.contains("task-123"));
    }

    #[tokio::test]
    async fn paginates_filters_tasks_and_uses_only_trusted_reply_socket() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = vec![
            task("foreign-actor", "peer-a", "pending", 0, None).tap_actor("other"),
            task("foreign-requester", "peer-a", "pending", 0, None).tap_requester("other-route"),
            task("target-task", "peer-b", "pending", 0, None),
        ];
        let _ = create_store(&dir.path().join("sic.sqlite"), &[tasks[2].clone()]).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(MockRunner {
            calls: calls.clone(),
            seen: seen.clone(),
            fail: false,
        });
        let (mut watch, api, server) = watcher(dir.path(), tasks, 2, runner).await;
        let outcome = watch.cycle().await.unwrap();
        assert_eq!(
            outcome,
            CycleOutcome::Replied {
                task_id: "target-task".into(),
                reply_message_id: "reply-message-fixture".into()
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let requests = api.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["payload"]["offset"], 0);
        assert_eq!(requests[1]["payload"]["offset"], 2);
        let observed = seen.lock().await;
        let row = &observed[0];
        assert_eq!(row.0, "target-task");
        assert_eq!(row.1, "peer-b");
        assert_eq!(row.4, "trusted-reply-socket");
        let ids = deterministic_control_ids("fixture-watch", "target-task").unwrap();
        assert_eq!(row.2, ids.claim_id);
        assert_eq!(row.3, ids.prepare_id);
        server.abort();
    }

    #[tokio::test]
    async fn restart_does_not_dispatch_claimed_task_without_process_intent() {
        let dir = tempfile::tempdir().unwrap();
        let pending = task("recover-task", "peer-a", "pending", 0, None);
        let store = create_store(&dir.path().join("sic.sqlite"), &[pending.clone()]).await;
        drop(store);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let failing_runner = Arc::new(MockRunner {
            calls: calls.clone(),
            seen: seen.clone(),
            fail: true,
        });
        let (mut first, api_state, server) =
            watcher(dir.path(), vec![pending.clone()], 10, failing_runner).await;
        assert!(matches!(
            first.cycle().await.unwrap(),
            CycleOutcome::Blocked { .. }
        ));
        drop(first);
        let ids = deterministic_control_ids("fixture-watch", "recover-task").unwrap();
        *api_state.tasks.lock().await = vec![task(
            "recover-task",
            "peer-a",
            "claimed",
            1,
            Some(&ids.claim_id),
        )];
        let store = Store::open(&dir.path().join("sic.sqlite")).await.unwrap();
        let mut tx = store.begin().await.unwrap();
        sqlx::query("UPDATE sic_peer_tasks SET state='claimed',claim_id=?,claimed_by='watcher-owner',revision=1 WHERE task_id='recover-task'")
            .bind(&ids.claim_id).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        drop(store);

        let (mut restarted, _, restarted_server) = watcher(
            dir.path(),
            api_state.tasks.lock().await.clone(),
            10,
            Arc::new(MockRunner {
                calls: calls.clone(),
                seen,
                fail: false,
            }),
        )
        .await;
        let outcome = restarted.cycle().await.unwrap();
        assert!(matches!(
            outcome,
            CycleOutcome::Blocked {
                reason: "claim_has_no_process_intent",
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
        restarted_server.abort();
    }

    #[tokio::test]
    async fn running_process_is_never_relaunched_by_another_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let ids = deterministic_control_ids("fixture-watch", "running-task").unwrap();
        let prepared = task("running-task", "peer-a", "prepared", 2, Some(&ids.claim_id));
        let store = create_store(&dir.path().join("sic.sqlite"), &[prepared.clone()]).await;
        let mut tx = store.begin().await.unwrap();
        sqlx::query("CREATE TABLE sic_peer_process_launches (task_id TEXT PRIMARY KEY NOT NULL, launch_id TEXT NOT NULL UNIQUE, actor TEXT NOT NULL, peer_chain_id TEXT NOT NULL, claim_id TEXT NOT NULL, fingerprint TEXT NOT NULL, executable TEXT NOT NULL, state TEXT NOT NULL, exit_code INTEGER, reason TEXT)")
            .execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO sic_peer_process_launches(task_id,launch_id,actor,peer_chain_id,claim_id,fingerprint,executable,state) VALUES(?,?,?,?,?,?,?,'running')")
            .bind("running-task").bind(&ids.launch_id).bind("watcher-owner").bind("peer-a")
            .bind(&ids.claim_id).bind("fingerprint").bind("host-selected-executable")
            .execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        drop(store);

        let calls = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(MockRunner {
            calls: calls.clone(),
            seen: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        });
        let (mut watch, _, server) = watcher(dir.path(), vec![prepared], 10, runner).await;
        assert_eq!(
            watch.cycle().await.unwrap(),
            CycleOutcome::Blocked {
                task_id: "running-task".into(),
                revision: 2,
                reason: "process_outcome_unknown",
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let mut tx = watch.store.begin().await.unwrap();
        sqlx::query("UPDATE sic_peer_process_launches SET state='exited',exit_code=0 WHERE task_id='running-task'")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        // The cursor first completes its current pagination pass, then wraps.
        assert_eq!(watch.cycle().await.unwrap(), CycleOutcome::Idle);
        let recovered = watch.cycle().await.unwrap();
        assert!(
            matches!(
                recovered,
                CycleOutcome::Replied { ref task_id, .. } if task_id == "running-task"
            ),
            "unexpected recovery result: {recovered:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn blocked_task_does_not_starve_following_page_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ids = deterministic_control_ids("fixture-watch", "claimed-without-intent").unwrap();
        let claimed = task(
            "claimed-without-intent",
            "peer-a",
            "prepared",
            2,
            Some(&ids.claim_id),
        );
        let pending = task("next-pending", "peer-b", "pending", 0, None);
        let _ = create_store(
            &dir.path().join("sic.sqlite"),
            &[claimed.clone(), pending.clone()],
        )
        .await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(MockRunner {
            calls: calls.clone(),
            seen: seen.clone(),
            fail: false,
        });
        let (mut watch, _, server) = watcher(dir.path(), vec![claimed, pending], 1, runner).await;

        assert!(matches!(
            watch.cycle().await.unwrap(),
            CycleOutcome::Blocked {
                task_id,
                reason: "claim_has_no_process_intent",
                ..
            } if task_id == "claimed-without-intent"
        ));
        assert!(matches!(
            watch.cycle().await.unwrap(),
            CycleOutcome::Replied { task_id, .. } if task_id == "next-pending"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(seen.lock().await[0].0, "next-pending");
        server.abort();
    }

    #[tokio::test]
    async fn existing_reply_receipt_skips_runner() {
        let dir = tempfile::tempdir().unwrap();
        let ids = deterministic_control_ids("fixture-watch", "replied-task").unwrap();
        let task = task("replied-task", "peer-a", "prepared", 2, Some(&ids.claim_id));
        let store = create_store(&dir.path().join("sic.sqlite"), &[task.clone()]).await;
        let mut tx = store.begin().await.unwrap();
        sqlx::query("INSERT INTO sic_peer_task_replies(task_id,attempt_id,reply_id,fingerprint,result) VALUES(?,?,?,?,?)")
            .bind("replied-task").bind("attempt-1").bind(&ids.reply_id).bind("fingerprint")
            .bind(json!({"state":"reply_delivered","attempt_id":"attempt-1"}).to_string())
            .execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        drop(store);
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(MockRunner {
            calls: calls.clone(),
            seen: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        });
        let (mut watch, _, server) = watcher(dir.path(), vec![task], 10, runner).await;
        assert_eq!(
            watch.cycle().await.unwrap(),
            CycleOutcome::ReplyAlreadyPresent {
                task_id: "replied-task".into()
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn unchanged_watch_state_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(MockRunner {
            calls,
            seen: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        });
        let (watch, _, server) = watcher(dir.path(), Vec::new(), 10, runner).await;
        let mut tx = watch.store.begin().await.unwrap();
        sqlx::query("CREATE TABLE watch_update_counter (updates INTEGER NOT NULL)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO watch_update_counter(updates) VALUES(0)")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER count_watch_updates AFTER UPDATE ON sic_peer_watch_attempts BEGIN UPDATE watch_update_counter SET updates=updates+1; END")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        watch
            .save_watch_state("task-a", 3, "blocked", Some("still_running"))
            .await
            .unwrap();
        watch
            .save_watch_state("task-a", 3, "blocked", Some("still_running"))
            .await
            .unwrap();
        assert_eq!(watch_update_count(&watch.store).await, 0);

        watch
            .save_watch_state("task-a", 3, "blocked", Some("outcome_unknown"))
            .await
            .unwrap();
        assert_eq!(watch_update_count(&watch.store).await, 1);
        watch
            .save_watch_state("task-a", 3, "reply_present", None)
            .await
            .unwrap();
        assert_eq!(watch_update_count(&watch.store).await, 2);
        server.abort();
    }

    async fn watch_update_count(store: &Store) -> i64 {
        let mut tx = store.begin().await.unwrap();
        let count = sqlx::query_scalar("SELECT updates FROM watch_update_counter")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        count
    }

    #[tokio::test]
    async fn default_configuration_is_disabled_without_core_requests() {
        let dir = tempfile::tempdir().unwrap();
        let task = task("idle", "peer-a", "pending", 0, None);
        let _ = create_store(&dir.path().join("sic.sqlite"), &[task.clone()]).await;
        let (base, api, server) = start_api(vec![task]).await;
        let mut template = config(dir.path());
        template.db_path = dir.path().join("sic.sqlite");
        template.core_base_url = base;
        let mut watch = PeerWatch::new(WatchConfig::default(), template, "mock-bearer")
            .await
            .unwrap();
        assert_eq!(watch.cycle().await.unwrap(), CycleOutcome::Disabled);
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(true);
        watch.watch_loop(stop_rx).await.unwrap();
        drop(stop_tx);
        assert!(api.requests.lock().await.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn rejects_dense_polling_unbounded_cycles_and_invalid_api_deadlines() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner {
            calls: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        });
        let mut cfg = WatchConfig::default();
        cfg.enabled = true;
        cfg.namespace = "fixture-watch".into();
        cfg.poll_interval = Duration::from_millis(100);
        let template = config(dir.path());
        assert!(matches!(
            PeerWatch::with_runner(cfg, template, "mock-token", runner.clone()).await,
            Err(WatchError::InvalidConfiguration)
        ));

        let mut cfg = WatchConfig::default();
        cfg.enabled = true;
        cfg.namespace = "fixture-watch".into();
        cfg.max_tasks_per_cycle = 2;
        let mut template = config(dir.path());
        template.api_timeout_seconds = 0;
        assert!(matches!(
            PeerWatch::with_runner(cfg, template, "mock-token", runner).await,
            Err(WatchError::InvalidConfiguration)
        ));
    }

    trait TestValueExt {
        fn tap_actor(self, actor: &str) -> Self;
        fn tap_requester(self, requester: &str) -> Self;
    }
    impl TestValueExt for Value {
        fn tap_actor(mut self, actor: &str) -> Self {
            self["actor"] = json!(actor);
            self
        }
        fn tap_requester(mut self, requester: &str) -> Self {
            self["requester_chain_id"] = json!(requester);
            self
        }
    }
}
