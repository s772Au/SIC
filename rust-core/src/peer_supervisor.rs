//! Trusted, single-task DSH peer supervisor.
//!
//! All launch settings are supplied by the host. The model controls neither
//! the executable nor its environment, the reply Socket, or the API identity.
//! This supervisor performs no retries after an ambiguous claim or launch.

use crate::{
    peer_process::{self, HostProcessSpec},
    peer_runner_client::{HostPermissions, PeerRunnerClient},
    peer_tasks,
    storage::{self, Store},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorConfig {
    pub db_path: PathBuf,
    pub core_base_url: String,
    pub bridge_base_url: String,
    pub bearer_env: String,
    pub actor: String,
    pub requester_chain_id: String,
    pub peer_chain_id: String,
    pub task_id: String,
    pub reply_socket_id: String,
    pub claim_id: String,
    pub prepare_id: String,
    pub launch_id: String,
    pub reply_id: String,
    pub policy: Value,
    pub dsh_executable: PathBuf,
    pub dsh_cli_js: PathBuf,
    pub dsh_home: PathBuf,
    pub dsh_profile: String,
    pub working_directory: PathBuf,
    pub state_directory: PathBuf,
    pub model_alias: String,
    pub max_tokens: u64,
    pub timeout_seconds: u64,
    pub api_timeout_seconds: u64,
    pub prices: Prices,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allow_all_tools: bool,
    #[serde(default)]
    pub fixture_read_tool: bool,
    pub prompt: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Serialize)]
pub struct SupervisorOutcome {
    pub state: &'static str,
    pub task_id: String,
    pub peer_chain_id: String,
    pub attempt_id: String,
    pub reply_message_id: String,
    pub replayed: bool,
    pub requester_adopted_cb: bool,
}

#[derive(Debug)]
pub struct SupervisorError(pub &'static str);

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for SupervisorError {}
type Result<T> = std::result::Result<T, SupervisorError>;

fn nonempty(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn loopback_bridge(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let authority = value
        .split_once("//")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| {
                ip == IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                    || ip == IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
            })
        && url.username().is_empty()
        && url.password().is_none()
        && authority.is_some_and(|part| !part.contains('@'))
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().trim_end_matches('/') == "/v1"
}

/// Check host-controlled settings before claiming any peer task.
pub fn validate(c: &SupervisorConfig) -> Result<()> {
    for id in [
        &c.actor,
        &c.requester_chain_id,
        &c.peer_chain_id,
        &c.task_id,
        &c.reply_socket_id,
        &c.claim_id,
        &c.prepare_id,
        &c.launch_id,
        &c.reply_id,
    ] {
        if !nonempty(id, 128) {
            return Err(SupervisorError("invalid_identity"));
        }
    }
    if !c
        .bearer_env
        .bytes()
        .enumerate()
        .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
        || c.bearer_env.is_empty()
        || c.bearer_env.len() > 128
        || [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "TEMP",
            "TMP",
            "DSH_HOME",
            "ELECTRON_RUN_AS_NODE",
        ]
        .contains(&c.bearer_env.to_ascii_uppercase().as_str())
        || !nonempty(&c.dsh_profile, 128)
        || !c
            .dsh_profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || !nonempty(&c.model_alias, 128)
        || !nonempty(&c.prompt, 8192)
        || !loopback_bridge(&c.bridge_base_url)
        || c.max_tokens == 0
        || c.max_tokens > 393_216
        || c.timeout_seconds == 0
        || c.timeout_seconds > 3600
        || c.api_timeout_seconds == 0
        || c.api_timeout_seconds > 3600
        || !c.policy.is_object()
    {
        return Err(SupervisorError("invalid_configuration"));
    }
    for path in [
        &c.db_path,
        &c.dsh_executable,
        &c.dsh_cli_js,
        &c.dsh_home,
        &c.working_directory,
        &c.state_directory,
    ] {
        if !path.is_absolute() || path.to_str().is_none_or(|s| s.contains('\0')) {
            return Err(SupervisorError("invalid_host_path"));
        }
    }
    if !c.db_path.is_file()
        || !c.dsh_executable.is_file()
        || !c.dsh_home.is_dir()
        || !c.working_directory.is_dir()
        || !c.state_directory.is_dir()
        || c.dsh_cli_js.extension().and_then(|s| s.to_str()) != Some("js")
    {
        return Err(SupervisorError("invalid_host_path"));
    }
    if c.allowed_tools.len() > 4096
        || c.allowed_tools.iter().any(|tool| !nonempty(tool, 128))
        || (c.allowed_tools.is_empty() && !c.allow_all_tools)
        || (c.allow_all_tools && !c.allowed_tools.is_empty())
    {
        return Err(SupervisorError("invalid_tool_allowlist"));
    }
    if [
        c.prices.input,
        c.prices.output,
        c.prices.cache_read,
        c.prices.cache_write,
    ]
    .iter()
    .any(|p| !p.is_finite() || *p < 0.0)
    {
        return Err(SupervisorError("invalid_prices"));
    }
    let policy: crate::scheduler::SchedulerPolicy = serde_json::from_value(c.policy.clone())
        .map_err(|_| SupervisorError("invalid_prepare_policy"))?;
    crate::core::validate_peer_prepare_policy(&policy)
        .map_err(|_| SupervisorError("invalid_prepare_policy"))?;
    Ok(())
}

fn patch_file(c: &SupervisorConfig) -> Result<PathBuf> {
    // JSON is a YAML subset, so user-supplied values are serialized rather than
    // interpolated into a shell or YAML fragment.
    let patch = json!([
        {"id":"agent-default-model","config":{"provider":"sic-core-rust","model":c.model_alias}},
        {"id":"sic-core-rust-dsh","config":{
            "provider":"sic-core-rust","baseURL":c.bridge_base_url,"apiKeyEnv":c.bearer_env,
            "model":c.model_alias,"bindChainId":c.peer_chain_id,"maxTokens":c.max_tokens,
            "inputPricePerMillion":c.prices.input,"outputPricePerMillion":c.prices.output,
            "cacheReadPricePerMillion":c.prices.cache_read,"cacheWritePricePerMillion":c.prices.cache_write,
            "allowedTools":c.allowed_tools,"fixtureReadTool":c.fixture_read_tool
        }},
        // Background title and default-provider integrations can start extra
        // requests on the same session or route around the fixed SIC model.
        {"id":"session-title-llm","name":"@deepseek-ai/dsh-session-title-first-prompt-llm","disabled":true},
        {"id":"llm-deepseek","name":"@deepseek-ai/dsh-llm-deepseek-api-key","disabled":true},
        {"id":"llm-deepseek-account","name":"@deepseek-ai/dsh-llm-deepseek-account","disabled":true},
        {"id":"web-search-deepseek","name":"@deepseek-ai/dsh-web-search-deepseek","disabled":true},
        {"id":"web-deepseek-api","name":"@deepseek-ai/dsh-web-deepseek-api","disabled":true},
        {"id":"session-telemetry-otel","name":"@deepseek-ai/dsh-session-telemetry-otel","disabled":true},
        {"id":"otel","name":"@deepseek-ai/dsh-otel","disabled":true}
    ]);
    let bytes = serde_json::to_vec(&patch).map_err(|_| SupervisorError("patch_encoding_failed"))?;
    let key = storage::digest(&json!([c.task_id, c.launch_id]))
        .map_err(|_| SupervisorError("patch_encoding_failed"))?;
    let root =
        fs::canonicalize(&c.state_directory).map_err(|_| SupervisorError("invalid_host_path"))?;
    let path = root.join(format!("sic-peer-{key}.json"));
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(&bytes)
                .map_err(|_| SupervisorError("patch_write_failed"))?;
            file.sync_all()
                .map_err(|_| SupervisorError("patch_write_failed"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| SupervisorError("patch_conflict"))?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || fs::read(&path).map_err(|_| SupervisorError("patch_conflict"))? != bytes
            {
                return Err(SupervisorError("patch_conflict"));
            }
        }
        Err(_) => return Err(SupervisorError("patch_write_failed")),
    }
    Ok(path)
}

fn process_spec(c: &SupervisorConfig, bearer: &str, patch: &Path) -> HostProcessSpec {
    let mut env = BTreeMap::new();
    for name in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(name) {
            env.insert(name.to_owned(), value);
        }
    }
    env.insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
    env.insert("DSH_HOME".into(), c.dsh_home.to_string_lossy().into_owned());
    env.insert(c.bearer_env.clone(), bearer.to_owned());
    HostProcessSpec {
        executable: c.dsh_executable.clone(),
        argv: vec![
            "--expose-internals".into(),
            c.dsh_cli_js.to_string_lossy().into_owned(),
            "--profile".into(),
            c.dsh_profile.clone(),
            "--patch".into(),
            patch.to_string_lossy().into_owned(),
            c.prompt.clone(),
        ],
        environment: env,
        working_directory: Some(c.working_directory.clone()),
        timeout: Duration::from_secs(c.timeout_seconds),
    }
}

pub(crate) async fn previous_reply_attempt(
    store: &Store,
    c: &SupervisorConfig,
) -> Result<Option<String>> {
    let mut tx = store
        .begin()
        .await
        .map_err(|_| SupervisorError("storage_failed"))?;
    let task =
        peer_tasks::get_scoped_in_transaction(&mut tx, &c.actor, &c.requester_chain_id, &c.task_id)
            .await
            .map_err(|_| SupervisorError("task_access_denied"))?;
    if task["requester_chain_id"] != c.requester_chain_id
        || task["peer_chain_id"] != c.peer_chain_id
    {
        return Err(SupervisorError("task_identity_mismatch"));
    }
    let row =
        sqlx::query("SELECT attempt_id FROM sic_peer_task_replies WHERE task_id=? AND reply_id=?")
            .bind(&c.task_id)
            .bind(&c.reply_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| SupervisorError("storage_failed"))?;
    let attempt = row
        .map(|r| r.try_get::<String, _>("attempt_id"))
        .transpose()
        .map_err(|_| SupervisorError("storage_failed"))?;
    tx.commit()
        .await
        .map_err(|_| SupervisorError("storage_failed"))?;
    Ok(attempt)
}

/// Run one explicitly authorized peer task. The task and all control IDs are
/// supplied by a trusted host. A replay of a lost claim cannot start a process.
pub async fn run(c: &SupervisorConfig, bearer: String) -> Result<SupervisorOutcome> {
    validate(c)?;
    let client = PeerRunnerClient::new_with_timeout(
        &c.core_base_url,
        bearer.clone(),
        &c.actor,
        HostPermissions::new(true, true, false),
        Duration::from_secs(c.api_timeout_seconds),
    )
    .map_err(|_| SupervisorError("invalid_core_client"))?;
    let store = Store::open(&c.db_path)
        .await
        .map_err(|_| SupervisorError("storage_failed"))?;
    let patch = patch_file(c)?;
    let spec = process_spec(c, &bearer, &patch);
    peer_process::validate_host_process_spec(&spec)
        .map_err(|_| SupervisorError("invalid_host_process"))?;
    let claim = client
        .operate(
            "peer.task.claim",
            Some(&c.requester_chain_id),
            &json!({"task_id":c.task_id,"claim_id":c.claim_id}),
        )
        .await
        .map_err(|_| SupervisorError("claim_unconfirmed"))?;
    let task = &claim["result"];
    if task["task_id"] != c.task_id
        || task["claim_id"] != c.claim_id
        || task["requester_chain_id"] != c.requester_chain_id
        || task["peer_chain_id"] != c.peer_chain_id
    {
        return Err(SupervisorError("task_identity_mismatch"));
    }
    let first_claim = task["execution_permit"] == true && task["replayed"] == false;
    let launched = if first_claim {
        let revision = task["revision"]
            .as_u64()
            .ok_or(SupervisorError("claim_unconfirmed"))?;
        let prepared = client
            .operate(
                "peer.task.prepare",
                Some(&c.requester_chain_id),
                &json!({
                    "task_id":c.task_id,"claim_id":c.claim_id,"expected_revision":revision,
                    "prepare_id":c.prepare_id,"policy":c.policy
                }),
            )
            .await
            .map_err(|_| SupervisorError("prepare_unconfirmed"))?;
        if prepared["result"]["state"] != "prepared"
            || prepared["result"]["ready_for_dsh"] != true
            || prepared["result"]["peer_chain_id"] != c.peer_chain_id
        {
            return Err(SupervisorError("peer_not_prepared"));
        }
        peer_process::launch_prepared_peer(
            &store,
            &c.actor,
            &c.peer_chain_id,
            &c.task_id,
            &c.claim_id,
            &c.launch_id,
            &spec,
        )
        .await
        .map_err(|_| SupervisorError("launch_unconfirmed"))?
    } else {
        // A reused claim never authorizes a first launch. Only an already
        // durably completed process may proceed to idempotent reply recovery.
        peer_process::inspect_launch_with_spec(
            &store,
            &c.actor,
            &c.peer_chain_id,
            &c.task_id,
            &c.claim_id,
            &c.launch_id,
            &spec,
        )
        .await
        .map_err(|_| SupervisorError("launch_unconfirmed"))?
        .ok_or(SupervisorError("claim_already_used"))?
    };
    if launched.state != "exited" || launched.exit_code != Some(0) {
        return Err(SupervisorError("process_outcome_not_successful"));
    }
    let attempt_id = match previous_reply_attempt(&store, c).await? {
        Some(id) => id,
        None => {
            let peer = client
                .operate("chain.get", Some(&c.peer_chain_id), &json!({}))
                .await
                .map_err(|_| SupervisorError("peer_status_unconfirmed"))?;
            peer["result"]["events"]
                .as_array()
                .and_then(|events| {
                    events
                        .iter()
                        .rev()
                        .find(|event| event["type"] == "worker_attempt")
                })
                .and_then(|event| event["record_id"].as_str())
                .filter(|id| nonempty(id, 128))
                .ok_or(SupervisorError("worker_attempt_missing"))?
                .to_owned()
        }
    };
    let reply = client.operate("peer.task.reply",Some(&c.requester_chain_id),&json!({
        "task_id":c.task_id,"reply_id":c.reply_id,"expected_attempt_id":attempt_id,"socket_id":c.reply_socket_id
    })).await.map_err(|_|SupervisorError("reply_unconfirmed"))?;
    let result = &reply["result"];
    if result["state"] != "reply_delivered"
        || result["message_delivered"] != true
        || result["requester_adopted_cb"] != false
        || result["attempt_id"] != attempt_id
    {
        return Err(SupervisorError("reply_unconfirmed"));
    }
    let message_id = result["receipt"]["message_id"]
        .as_str()
        .filter(|id| nonempty(id, 256))
        .ok_or(SupervisorError("reply_unconfirmed"))?
        .to_owned();
    Ok(SupervisorOutcome {
        state: "reply_delivered",
        task_id: c.task_id.clone(),
        peer_chain_id: c.peer_chain_id.clone(),
        attempt_id,
        reply_message_id: message_id,
        replayed: !first_claim,
        requester_adopted_cb: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path) -> SupervisorConfig {
        let db = dir.join("fixture.sqlite");
        std::fs::write(&db, b"fixture").unwrap();
        serde_json::from_value(json!({
            "db_path":db,"core_base_url":"http://127.0.0.1:1234",
            "bridge_base_url":"http://127.0.0.1:1234/v1","bearer_env":"SIC_CORE_TOKEN",
            "actor":"fixture-owner","requester_chain_id":"requester","peer_chain_id":"peer",
            "task_id":"task","reply_socket_id":"socket","claim_id":"claim","prepare_id":"prepare",
            "launch_id":"launch","reply_id":"reply",
            "policy":{"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":1,
                "max_units_per_wb":64,"level_batch_sizes":{},"max_level":1,"flush":true,
                "max_actions":32,"max_model_calls":4,"editor_mode":"provider","program_rules":[]},
            "dsh_executable":std::env::current_exe().unwrap(),"dsh_cli_js":dir.join("cli.js"),
            "dsh_home":dir,"dsh_profile":"fixture-profile","working_directory":dir,
            "state_directory":dir,"model_alias":"sic-worker","max_tokens":4096,
            "timeout_seconds":60,"api_timeout_seconds":180,
            "prices":{"input":0,"output":0,"cache_read":0,"cache_write":0},
            "allowed_tools":["fixture"],"prompt":"local fixture prompt"
        }))
        .unwrap()
    }

    #[test]
    fn validates_loopback_origin_tool_grants_and_policy_before_claim() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = config(temp.path());
        assert!(validate(&c).is_ok());
        assert!(loopback_bridge("http://[::1]:1234/v1"));
        for endpoint in [
            "https://127.0.0.1:1234/v1",
            "http://example.com/v1",
            "http://127.0.0.1:1234/v1?x=1",
            "http://u@127.0.0.1:1234/v1",
            "http://127.0.0.1:1234/v1/other",
        ] {
            c.bridge_base_url = endpoint.into();
            assert!(validate(&c).is_err());
        }
        c.bridge_base_url = "http://127.0.0.1:1234/v1".into();
        c.bearer_env = "DSH_HOME".into();
        assert!(validate(&c).is_err());
        c.bearer_env = "SIC_CORE_TOKEN".into();
        c.allowed_tools.clear();
        assert!(validate(&c).is_err());
        c.allow_all_tools = true;
        assert!(validate(&c).is_ok());
        c.policy["max_model_calls"] = json!(99);
        assert_eq!(validate(&c).unwrap_err().0, "invalid_prepare_policy");
    }

    #[test]
    fn patch_is_deterministic_and_never_contains_bearer_or_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let c = config(temp.path());
        let path = patch_file(&c).unwrap();
        assert_eq!(patch_file(&c).unwrap(), path);
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains(&c.prompt));
        assert!(!text.contains("fixture-secret-value"));
        let patch: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            patch
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == "session-title-llm" && entry["disabled"] == true)
        );
        fs::write(&path, b"different").unwrap();
        assert_eq!(patch_file(&c).unwrap_err().0, "patch_conflict");
    }
}
