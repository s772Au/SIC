mod model_policy {
    pub use sic_core_rust::model_policy::*;
}

mod core {
    pub use sic_core_rust::core::*;
}
mod storage {
    pub use sic_core_rust::storage::*;
}
#[path = "../src/dsh_bridge.rs"]
mod dsh_bridge;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use dsh_bridge::{DshBridge, DshBridgeConfig, DshRuntime};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[derive(Default)]
struct MockRuntime {
    calls: Mutex<Vec<(String, Option<String>, Value, bool)>>,
    pause_on: Option<String>,
    fail_claim_task: Option<String>,
    fail_observation_once: AtomicBool,
    fail_edit_run_once: AtomicBool,
    chain_pending: Option<Value>,
    claim_status_override: Option<String>,
    claimed_tasks: Arc<Mutex<HashSet<String>>>,
    started: Arc<Notify>,
    worker_script: Mutex<VecDeque<Value>>,
}

impl DshRuntime for MockRuntime {
    fn operate(
        &self,
        _actor: String,
        chain_id: Option<String>,
        operation: String,
        payload: Value,
        allow_model: bool,
    ) -> dsh_bridge::BoxFuture<Result<Value, String>> {
        let claim_id = payload.get("claim_id").cloned().unwrap_or(Value::Null);
        let completion_status = if payload.get("delivery").is_some() {
            "delivered_unattested"
        } else {
            "completed"
        };
        let wb_source_id = payload
            .get("source_ids")
            .and_then(Value::as_array)
            .and_then(|ids| ids.first())
            .cloned()
            .unwrap_or(Value::Null);
        let wb_source_roots = payload.get("source_ids").cloned().unwrap_or(Value::Null);
        let claim_task_id = payload
            .get("task_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let fail_claim = operation == "tool.claim"
            && self.fail_claim_task.as_deref() == claim_task_id.as_deref();
        let fail_observation =
            operation == "tool.observe" && self.fail_observation_once.swap(false, Ordering::SeqCst);
        let fail_edit_run =
            operation == "edit.run" && self.fail_edit_run_once.swap(false, Ordering::SeqCst);
        let chain_pending = self.chain_pending.clone();
        let claim_status = self
            .claim_status_override
            .clone()
            .unwrap_or_else(|| "claimed".into());
        let claimed_tasks = self.claimed_tasks.clone();
        let mut calls = self.calls.lock().unwrap();
        let n = calls
            .iter()
            .filter(|(name, _, _, _)| name == &operation)
            .count()
            + 1;
        calls.push((operation.clone(), chain_id, payload, allow_model));
        drop(calls);
        let scripted = if operation == "worker.run" {
            self.worker_script.lock().unwrap().pop_front()
        } else {
            None
        };
        let should_pause = self.pause_on.as_deref() == Some(operation.as_str());
        let started = self.started.clone();
        Box::pin(async move {
            if should_pause {
                started.notify_one();
                future::pending::<()>().await;
            }
            if fail_claim {
                return Err("mock claim response unavailable".to_owned());
            }
            if fail_observation {
                return Err("mock observation interruption".to_owned());
            }
            if fail_edit_run {
                return Err("mock edit provider 500".to_owned());
            }
            let replayed = if operation == "tool.claim" {
                claim_task_id
                    .as_ref()
                    .is_some_and(|task_id| !claimed_tasks.lock().unwrap().insert(task_id.clone()))
            } else {
                false
            };
            let result = match operation.as_str() {
                "chain.create" => json!({"chain_id":"chain-test"}),
                "tools.configure" | "tools.sync" => json!({"configured":true}),
                "tool.claim" => json!({"task_id":claim_task_id,"status":claim_status,"claim_id":claim_id}),
                "tool.complete" => json!({"task_id":"task-1","status":completion_status,"source_id":"worker-1","open_calls":[]}),
                "tool.observe" => json!({"recorded":true}),
                "source.ingest" => json!({"source_id":format!("source-{n}")}),
                "wb.form" => json!({"wb_id":format!("wb-{n}"),"source_id":wb_source_id,"source_roots":wb_source_roots}),
                "edit.prepare" => json!({"job_id":format!("job-{n}")}),
                "edit.run" => json!({"status":"ready"}),
                "chain.get" => chain_pending.clone().unwrap_or_else(|| json!({"cut_seq":n - 1})),
                "cut.publish" => json!({"cut_id":format!("cut-{n}"),"cb_id":format!("cb-{n}")}),
                "accounting.get" => json!({"groups":[{"role":"editor","attempts":1}],"complete":false,"attempts_with_unknown_usage":1,"unresolved_attempts":1,"pending_worker_actions":1,"superseded_worker_actions":1}),
                "worker.run" => scripted.unwrap_or_else(|| json!({"content":format!("mock answer {n}"),"source_id":format!("worker-{n}"),"tool_calls":[],"tool_results":[],"host_tasks":[]})),
                _ => return Err("unsupported mock operation".to_owned()),
            };
            Ok(
                json!({"ok":true,"operation":operation,"result":result,"evidence":{"replayed":replayed}}),
            )
        })
    }
}

#[tokio::test]
async fn native_events_are_scoped_idempotent_and_reach_matching_tool_continuations() {
    for (outcome, expected) in [
        ("ok", json!(true)),
        ("error", json!(false)),
        ("not_started", json!(false)),
        ("unknown", Value::Null),
    ] {
        let store = sic_core_rust::storage::Store::memory().await.unwrap();
        let call = json!({"id":"native-call","type":"function","function":{
            "name":"sic_tool_call","arguments":"{\"name\":\"weather_lookup\",\"arguments\":{\"city\":\"Suzhou\"}}"}});
        let task = json!({"task_id":"native-task","chain_id":"chain-test","actor":"owner","status":"pending",
            "source_id":"worker-tool-source","call_id":"native-call",
            "descriptor":{"name":"weather_lookup","description":"Look up local weather",
                "schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}},
            "arguments":{"city":"Suzhou"}});
        let runtime = Arc::new(MockRuntime {
            fail_observation_once: AtomicBool::new(outcome == "error"),
            worker_script: Mutex::new(VecDeque::from([
                json!({"content":null,"tool_calls":[call],"tool_results":[],"host_tasks":[task],"source_id":"worker-tool-source","attempt_id":"worker-1"}),
                json!({"content":"continued","tool_calls":[],"tool_results":[],"host_tasks":[],"source_id":"worker-final","attempt_id":"worker-2"}),
            ])),
            ..Default::default()
        });
        let mut bridge = make_bridge(runtime.clone(), store.clone()).await;
        let mut first = body(json!([{"role":"user","content":"private task"}]));
        first["tools"] = harness_tools();
        bridge
            .handle("owner", &headers("event-session"), &first)
            .await
            .unwrap();
        assert_eq!(
            bridge
                .record_native_tool_event("other", "event-session", "native-call", 42, outcome)
                .await
                .unwrap_err()
                .status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bridge
                .record_native_tool_event("owner", "event-session", "wrong-call", 42, outcome)
                .await
                .unwrap_err()
                .status,
            StatusCode::NOT_FOUND
        );
        let recorded = bridge
            .record_native_tool_event("owner", "event-session", "native-call", 42, outcome)
            .await;
        if outcome == "error" {
            assert_eq!(recorded.unwrap_err().status, StatusCode::BAD_GATEWAY);
            assert_eq!(
                bridge
                    .session_tool_events("owner", "event-session", None, 50)
                    .await
                    .unwrap()["items"][0]["core_observed"],
                false
            );
            drop(bridge);
            bridge = make_bridge(runtime.clone(), store.clone()).await;
        } else {
            assert_eq!(recorded.unwrap()["replayed"], false);
            assert_eq!(
                bridge
                    .record_native_tool_event("owner", "event-session", "native-call", 42, outcome)
                    .await
                    .unwrap()["replayed"],
                true
            );
        }
        assert_eq!(
            bridge
                .record_native_tool_event("owner", "event-session", "native-call", 43, outcome)
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        let before = bridge
            .session_tool_events("owner", "event-session", None, 50)
            .await
            .unwrap();
        assert_eq!(before["items"][0]["native_outcome"], outcome);
        assert_eq!(before["items"][0]["core_observed"], outcome != "error");
        assert_eq!(before["items"][0]["native_event_seq"], 42);
        assert!(!before.to_string().contains("Suzhou"));

        let mut continuation = body(json!([
            {"role":"user","content":"private task"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"native-call","type":"function",
                "function":{"name":"weather_lookup","arguments":"{\"city\":\"Suzhou\"}"}}]},
            {"role":"tool","tool_call_id":"native-call","name":"weather_lookup","content":"private result"}
        ]));
        continuation["tools"] = harness_tools();
        bridge
            .handle("owner", &headers("event-session"), &continuation)
            .await
            .unwrap();
        let calls = runtime.calls.lock().unwrap();
        let completion = calls.iter().find(|c| c.0 == "tool.complete").unwrap();
        assert_eq!(completion.2["succeeded"], expected);
        assert_eq!(completion.2.get("delivery").is_some(), outcome == "unknown");
        drop(calls);
        let after = bridge
            .session_tool_events("owner", "event-session", None, 50)
            .await
            .unwrap();
        assert_eq!(after["items"][0]["task_state"], "completed");
        assert_eq!(after["items"][0]["core_observed"], true);
        assert_eq!(after["items"][0]["completion_outcome"], outcome);
        assert!(!after.to_string().contains("private result"));
        drop(bridge);
        let reopened = make_bridge(runtime, store).await;
        assert_eq!(
            reopened
                .session_tool_events("owner", "event-session", None, 50)
                .await
                .unwrap()["items"][0]["native_outcome"],
            outcome
        );
    }
}

fn headers(session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-sic-bridge-session",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

#[tokio::test]
async fn native_event_schema_migrates_older_tool_rows_without_inventing_an_outcome() {
    use sqlx::{
        Row,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    let dir = tempdir().unwrap();
    let path = dir.path().join("previous-bridge.sqlite");
    let store = sic_core_rust::storage::Store::create(&path).await.unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TABLE dsh_bridge_tool_tasks(actor TEXT NOT NULL,session_key TEXT NOT NULL,call_id TEXT NOT NULL,task_id TEXT NOT NULL,tool_name TEXT NOT NULL,arguments TEXT NOT NULL,task_state TEXT NOT NULL,result_fingerprint TEXT,created INTEGER NOT NULL,PRIMARY KEY(actor,session_key,call_id),UNIQUE(actor,session_key,task_id))")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO dsh_bridge_tool_tasks VALUES('old-owner','old-session','old-call','old-task','read','{}','completed','old-fingerprint',1)")
        .execute(&pool).await.unwrap();
    let bridge = make_bridge(Arc::new(MockRuntime::default()), store).await;
    let row = sqlx::query("SELECT task_id,result_fingerprint,completion_outcome FROM dsh_bridge_tool_tasks WHERE call_id='old-call'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.try_get::<String, _>("task_id").unwrap(), "old-task");
    assert_eq!(
        row.try_get::<String, _>("result_fingerprint").unwrap(),
        "old-fingerprint"
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("completion_outcome")
            .unwrap(),
        None
    );
    drop(bridge);
    pool.close().await;
}

fn body(messages: Value) -> Value {
    json!({
        "model":"sic-worker",
        "messages":messages,
        "stream":true,
        "stream_options":{"include_usage":true},
        "tools":[],
        "tool_choice":"auto",
        "max_tokens":4096
    })
}

fn harness_tools() -> Value {
    json!([{"type":"function","function":{"name":"weather_lookup","description":"Look up local weather", "parameters":{
        "type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false
    }}}])
}

async fn make_bridge(
    runtime: Arc<MockRuntime>,
    store: sic_core_rust::storage::Store,
) -> Arc<DshBridge> {
    Arc::new(
        DshBridge::new(
            store,
            runtime,
            DshBridgeConfig {
                model_alias: "sic-worker".into(),
                fixed_max_tokens: Some(4096),
                max_worker_calls_per_turn: 8,
                max_host_tasks_per_response: 4,
                max_harness_tools: 64,
                fixed_reasoning_effort: None,
            },
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn runs_editor_publish_worker_once_and_replays_delivered_sse() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;
    let session_headers = headers("dsh-session-a");
    let first = body(json!([
        {"role":"system","content":"preserve facts"},
        {"role":"user","content":"first request"}
    ]));

    let response = bridge
        .handle("actor-a", &session_headers, &first)
        .await
        .unwrap();
    let response_text = String::from_utf8(response.clone()).unwrap();
    assert!(response_text.contains("mock answer 1"));
    assert!(response_text.ends_with("data: [DONE]\n\n"));
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>(),
        vec![
            "chain.create",
            "source.ingest",
            "wb.form",
            "edit.prepare",
            "edit.run",
            "chain.get",
            "cut.publish",
            "worker.run"
        ]
    );
    assert!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.0 == "edit.run" && call.3)
    );
    assert!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.0 == "worker.run" && call.3)
    );

    let replay = bridge
        .handle("actor-a", &session_headers, &first)
        .await
        .unwrap();
    assert_eq!(replay, response);
    assert_eq!(runtime.calls.lock().unwrap().len(), 8);

    let second = body(json!([
        {"role":"system","content":"preserve facts"},
        {"role":"user","content":"first request"},
        {"role":"assistant","content":"mock answer 1"},
        {"role":"user","content":"second request"}
    ]));
    let _ = bridge
        .handle("actor-a", &session_headers, &second)
        .await
        .unwrap();
    let calls = runtime.calls.lock().unwrap();
    let second_ingest = calls
        .iter()
        .filter(|call| call.0 == "source.ingest")
        .nth(1)
        .unwrap();
    assert_eq!(
        second_ingest.2["request"]["messages"],
        json!([{"role":"user","content":"second request"}])
    );
    let second_wb = calls
        .iter()
        .filter(|call| call.0 == "wb.form")
        .nth(1)
        .unwrap();
    assert_eq!(second_wb.2["source_ids"], json!(["worker-1", "source-2"]));
}

#[tokio::test]
async fn authenticated_harness_catalog_changes_are_synced_before_worker() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;
    let h = headers("changing-tools");
    let first = body(json!([{"role":"user","content":"first"}]));
    bridge.handle("actor-a", &h, &first).await.unwrap();
    let mut next = body(
        json!([{"role":"user","content":"first"},{"role":"assistant","content":"mock answer 1"},
                             {"role":"user","content":"second"}]),
    );
    next["tools"] = harness_tools();
    bridge.handle("actor-a", &h, &next).await.unwrap();
    let calls = runtime.calls.lock().unwrap();
    let sync = calls
        .iter()
        .position(|entry| entry.0 == "tools.sync")
        .unwrap();
    let last_worker = calls
        .iter()
        .rposition(|entry| entry.0 == "worker.run")
        .unwrap();
    assert!(sync < last_worker);
    assert_eq!(
        calls[sync].2["tools"][0]["descriptor"]["name"],
        "weather_lookup"
    );
}

#[tokio::test]
async fn same_session_concurrent_identical_turns_are_serialized_and_replayed() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;
    let h = headers("same-concurrent-session");
    let request = body(json!([{"role":"user","content":"one request"}]));
    let (first, second) = tokio::join!(
        bridge.handle("actor-a", &h, &request),
        bridge.handle("actor-a", &h, &request)
    );
    assert_eq!(first.unwrap(), second.unwrap());
    let calls = runtime.calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|entry| entry.0 == "edit.run").count(),
        1
    );
    assert_eq!(
        calls.iter().filter(|entry| entry.0 == "worker.run").count(),
        1
    );
}

#[tokio::test]
async fn rejects_tools_tool_calls_and_unhonored_controls_without_core_work() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;
    let h = headers("unsupported");
    let plain = body(json!([{"role":"user","content":"go"}]));

    let mut calls = plain.clone();
    calls["messages"] = json!([
        {"role":"user","content":"go"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"some_tool","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"call-1","name":"some_tool","content":"done"}
    ]);
    // The nonempty tools table itself is now supported, but tool-call history
    // is rejected unless it maps to a bridge-issued task for this session.
    assert_eq!(
        bridge.handle("actor", &h, &calls).await.unwrap_err().status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let mut reasoning = plain;
    reasoning["reasoning_effort"] = json!("high");
    assert_eq!(
        bridge
            .handle("actor", &h, &reasoning)
            .await
            .unwrap_err()
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut different_budget = body(json!([{"role":"user","content":"go"}]));
    different_budget["max_tokens"] = json!(2048);
    assert_eq!(
        bridge
            .handle("actor", &h, &different_budget)
            .await
            .unwrap_err()
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(runtime.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reserves_every_common_sic_tool_name_but_accepts_external_names() {
    let reserved = core::Core::common_tool_names();
    assert!(reserved.iter().any(|name| name == "sic_board_claim"));
    assert!(reserved.iter().any(|name| name == "sic_socket_spawn"));
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;

    for (index, name) in ["sic_board_claim", "sic_socket_spawn"]
        .into_iter()
        .enumerate()
    {
        let mut request = body(json!([{"role":"user","content":"continue"}]));
        request["tools"] = json!([{"type":"function","function":{
            "name":name,
            "description":"collision probe",
            "parameters":{"type":"object","properties":{},"additionalProperties":false}
        }}]);
        let error = bridge
            .handle(
                "actor-reserved",
                &headers(&format!("reserved-{index}")),
                &request,
            )
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(runtime.calls.lock().unwrap().is_empty());

    let mut ordinary = body(json!([{"role":"user","content":"continue"}]));
    ordinary["tools"] = json!([{"type":"function","function":{
        "name":"weather_lookup",
        "description":"ordinary host tool",
        "parameters":{"type":"object","properties":{},"additionalProperties":false}
    }}]);
    bridge
        .handle("actor-ordinary", &headers("ordinary"), &ordinary)
        .await
        .unwrap();
    let calls = runtime.calls.lock().unwrap();
    let configured = calls
        .iter()
        .find(|call| call.0 == "tools.configure")
        .expect("ordinary DSH tool should reach host configuration");
    assert_eq!(
        configured.2["tools"][0]["descriptor"]["name"],
        "weather_lookup"
    );
}

#[tokio::test]
async fn rejects_oversized_description_in_host_tool_catalog() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime.clone(), store).await;
    let mut request = body(json!([{"role":"user","content":"continue"}]));
    request["tools"] = json!([{"type":"function","function":{
        "name":"large_description_tool",
        "description":"x".repeat(1_048_577),
        "parameters":{"type":"object","properties":{},"additionalProperties":false}
    }}]);
    let error = bridge
        .handle(
            "actor-large-description",
            &headers("large-description"),
            &request,
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(runtime.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn host_task_roundtrip_records_received_output_without_claiming_success() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let first_call = json!({"id":"dsh-call-7","type":"function","function":{
        "name":"sic_tool_call","arguments":"{\"name\":\"weather_lookup\",\"arguments\":{\"city\":\"Suzhou\"}}"
    }});
    let descriptor = json!({
        "name":"weather_lookup","description":"Look up local weather",
        "schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}
    });
    let host_task = json!({
        "task_id":"task-durable-7","chain_id":"chain-test","actor":"actor-tools","status":"pending",
        "source_id":"worker-tool-source","call_id":"dsh-call-7","descriptor":descriptor,
        "arguments":{"city":"Suzhou"}
    });
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([
            json!({"content":null,"tool_calls":[first_call.clone()],"tool_results":[],"host_tasks":[host_task],"source_id":"worker-tool-source","attempt_id":"worker-attempt-1"}),
            json!({"content":"Weather result received.","tool_calls":[],"tool_results":[],"host_tasks":[],"source_id":"worker-final-source","attempt_id":"worker-attempt-2"}),
        ])),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store).await;
    let h = headers("dsh-tools");
    let mut initial = body(json!([{"role":"user","content":"Check the weather in Suzhou."}]));
    initial["tools"] = harness_tools();
    let tool_sse = bridge.handle("actor-tools", &h, &initial).await.unwrap();
    let tool_sse = String::from_utf8(tool_sse).unwrap();
    assert!(tool_sse.contains("dsh-call-7"));
    assert!(tool_sse.contains("weather_lookup"));
    assert!(tool_sse.contains("Suzhou"));
    let issued_status = bridge
        .session_status("actor-tools", "dsh-tools")
        .await
        .unwrap();
    assert_eq!(issued_status["last_turn"]["state"], "tool_issued");
    assert_eq!(issued_status["issued_host_tasks"], 1);
    assert_eq!(issued_status["last_publication"]["cb_id"], "cb-1");
    assert!(!issued_status.to_string().contains("Suzhou"));
    assert!(runtime.calls.lock().unwrap().iter().any(|call| {
        call.0 == "tools.configure"
            && call.2["acl"]["execute"][0]["principal"] == "actor-tools"
            && call.2["tools"][0]["descriptor"]["name"] == "weather_lookup"
    }));
    let issued_claim_id = {
        let calls = runtime.calls.lock().unwrap();
        let issued_claim = calls
            .iter()
            .find(|call| call.0 == "tool.claim")
            .expect("task must be claimed before it is returned");
        assert_eq!(issued_claim.2["task_id"], "task-durable-7");
        let value = issued_claim.2["claim_id"].as_str().unwrap().to_owned();
        assert!(value.starts_with("dsh:"));
        value
    };

    // An HTTP retry after issuing external calls is blocked, never re-emitted.
    assert_eq!(
        bridge
            .handle("actor-tools", &h, &initial)
            .await
            .unwrap_err()
            .code,
        "turn_incomplete"
    );

    let mut result_request = body(json!([
        {"role":"user","content":"Check the weather in Suzhou."},
        {"role":"assistant","content":null,"tool_calls":[{
            "id":"dsh-call-7","type":"function","function":{"name":"weather_lookup","arguments":"{ \"city\" : \"Suzhou\" }"}
        }]},
        {"role":"tool","tool_call_id":"dsh-call-7","name":"weather_lookup","content":"sunny, 19 C"}
    ]));
    result_request["tools"] = harness_tools();
    let final_sse = bridge
        .handle("actor-tools", &h, &result_request)
        .await
        .unwrap();
    let final_sse = String::from_utf8(final_sse).unwrap();
    assert!(final_sse.contains("Weather result received."));
    let completed_status = bridge
        .session_status("actor-tools", "dsh-tools")
        .await
        .unwrap();
    assert_eq!(completed_status["last_turn"]["state"], "delivered");
    assert_eq!(completed_status["issued_host_tasks"], 0);
    assert!(completed_status["last_turn"]["references"]["wb_id"].is_null());
    assert_eq!(completed_status["last_publication"]["wb_id"], "wb-1");
    assert_eq!(completed_status["last_publication"]["cb_id"], "cb-1");
    assert_eq!(
        completed_status["last_turn"]["tool_completions_recorded"],
        1
    );
    assert_ne!(issued_status["revision"], completed_status["revision"]);
    assert!(!completed_status.to_string().contains("sunny, 19 C"));
    let late = bridge
        .record_native_tool_event("actor-tools", "dsh-tools", "dsh-call-7", 51, "ok")
        .await
        .unwrap();
    assert_eq!(late["recorded"], true);
    let event_page = bridge
        .session_tool_events("actor-tools", "dsh-tools", None, 50)
        .await
        .unwrap();
    assert_eq!(event_page["items"][0]["native_outcome"], "ok");
    assert_eq!(event_page["items"][0]["completion_outcome"], "unknown");
    let calls = runtime.calls.lock().unwrap();
    let completions: Vec<_> = calls
        .iter()
        .filter(|call| call.0 == "tool.complete")
        .collect();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].2["task_id"], "task-durable-7");
    assert_eq!(completions[0].2["claim_id"], issued_claim_id);
    assert!(completions[0].2["succeeded"].is_null());
    assert_eq!(completions[0].2["delivery"], "received");
    assert_eq!(completions[0].2["result"], "sunny, 19 C");
    drop(calls);
    // Repeating this delivered result request replays its final SSE receipt.
    assert_eq!(
        bridge
            .handle("actor-tools", &h, &result_request)
            .await
            .unwrap(),
        final_sse.into_bytes()
    );
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.complete")
            .count(),
        1
    );
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.claim")
            .count(),
        1,
        "a delivered turn retry must not claim or reissue the task again"
    );
}

#[tokio::test]
async fn partial_host_task_claim_failure_quarantines_prior_claim_without_reissuing() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let tools = json!([
        {"type":"function","function":{"name":"tool_a","description":"A","parameters":{"type":"object","properties":{"v":{"type":"string"}},"required":["v"]}}},
        {"type":"function","function":{"name":"tool_b","description":"B","parameters":{"type":"object","properties":{"v":{"type":"string"}},"required":["v"]}}}
    ]);
    let descriptor_a = json!({"name":"tool_a","description":"A","schema":{"type":"object","properties":{"v":{"type":"string"}},"required":["v"]}});
    let descriptor_b = json!({"name":"tool_b","description":"B","schema":{"type":"object","properties":{"v":{"type":"string"}},"required":["v"]}});
    let call_a = json!({"id":"call-a","type":"function","function":{"name":"sic_tool_call","arguments":"{\"name\":\"tool_a\",\"arguments\":{\"v\":\"a\"}}"}});
    let call_b = json!({"id":"call-b","type":"function","function":{"name":"sic_tool_call","arguments":"{\"name\":\"tool_b\",\"arguments\":{\"v\":\"b\"}}"}});
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([json!({
            "content":null,"tool_calls":[call_a,call_b],"tool_results":[],"source_id":"worker-multi",
            "host_tasks":[
                {"task_id":"task-a","chain_id":"chain-test","actor":"actor-multi","status":"pending","source_id":"worker-multi","call_id":"call-a","descriptor":descriptor_a,"arguments":{"v":"a"}},
                {"task_id":"task-b","chain_id":"chain-test","actor":"actor-multi","status":"pending","source_id":"worker-multi","call_id":"call-b","descriptor":descriptor_b,"arguments":{"v":"b"}}
            ]
        })])),
        fail_claim_task: Some("task-b".into()),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store).await;
    let mut req = body(json!([{"role":"user","content":"run both tools"}]));
    req["tools"] = tools;
    let h = headers("partial-claims");
    let first = bridge.handle("actor-multi", &h, &req).await.unwrap_err();
    assert_eq!(first.code, "core_failed");
    assert!(runtime.claimed_tasks.lock().unwrap().contains("task-a"));
    assert!(!runtime.claimed_tasks.lock().unwrap().contains("task-b"));
    let calls_before = runtime.calls.lock().unwrap().len();
    let retry = bridge.handle("actor-multi", &h, &req).await.unwrap_err();
    assert_eq!(retry.code, "turn_indeterminate");
    assert_eq!(runtime.calls.lock().unwrap().len(), calls_before);
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.claim")
            .count(),
        2,
        "retry must not reissue either task after a partial claim"
    );
}

#[tokio::test]
async fn bridge_never_exposes_a_replayed_or_terminal_claim_to_dsh() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let call = json!({"id":"already-claimed-call","type":"function","function":{
        "name":"sic_tool_call","arguments":"{\"name\":\"weather_lookup\",\"arguments\":{\"city\":\"Osaka\"}}"
    }});
    let descriptor = json!({"name":"weather_lookup","description":"Look up local weather","schema":{
        "type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false
    }});
    let task = json!({"task_id":"task-durable-7","chain_id":"chain-test","actor":"actor-replay","status":"pending",
        "source_id":"source-replay","call_id":"already-claimed-call","descriptor":descriptor,"arguments":{"city":"Osaka"}});
    let mut claimed_tasks = HashSet::new();
    claimed_tasks.insert("task-durable-7".to_owned());
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([
            json!({"content":null,"tool_calls":[call],"tool_results":[],
            "host_tasks":[task],"source_id":"source-replay"}),
        ])),
        claimed_tasks: Arc::new(Mutex::new(claimed_tasks)),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store).await;
    let mut req = body(json!([{"role":"user","content":"check the weather"}]));
    req["tools"] = harness_tools();
    let error = bridge
        .handle("actor-replay", &headers("claim-replay"), &req)
        .await
        .unwrap_err();
    assert_eq!(error.code, "tool_claim_replayed");
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.claim")
            .count(),
        1
    );
    assert_eq!(
        bridge
            .handle("actor-replay", &headers("claim-replay"), &req)
            .await
            .unwrap_err()
            .code,
        "turn_indeterminate"
    );
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "worker.run")
            .count(),
        1
    );
}

#[tokio::test]
async fn bridge_requires_claim_result_status_to_be_claimed() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let call = json!({"id":"terminal-call","type":"function","function":{
        "name":"sic_tool_call","arguments":"{\"name\":\"weather_lookup\",\"arguments\":{\"city\":\"Osaka\"}}"
    }});
    let descriptor = json!({"name":"weather_lookup","description":"Look up local weather","schema":{
        "type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false
    }});
    let task = json!({"task_id":"task-terminal","chain_id":"chain-test","actor":"actor-terminal","status":"pending",
        "source_id":"source-terminal","call_id":"terminal-call","descriptor":descriptor,"arguments":{"city":"Osaka"}});
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([
            json!({"content":null,"tool_calls":[call],"tool_results":[],
            "host_tasks":[task],"source_id":"source-terminal"}),
        ])),
        claim_status_override: Some("completed".into()),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store).await;
    let mut req = body(json!([{"role":"user","content":"check the weather"}]));
    req["tools"] = harness_tools();
    let error = bridge
        .handle("actor-terminal", &headers("terminal-claim"), &req)
        .await
        .unwrap_err();
    assert_eq!(error.code, "tool_claim_replayed");
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.claim")
            .count(),
        1
    );
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "tool.complete")
            .count(),
        0
    );
}

#[tokio::test]
async fn internal_search_tool_rounds_continue_without_exposing_sic_control_calls_to_dsh() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let search_call = json!({"id":"internal-search-1","type":"function","function":{
        "name":"sic_tool_search","arguments":"{\"query\":\"weather\"}"
    }});
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([
            json!({"content":null,"tool_calls":[search_call],"tool_results":[{"tool_call_id":"internal-search-1","result":{"ok":true,"tools":[]}}],"host_tasks":[],"source_id":"worker-round-1"}),
            json!({"content":"Search complete.","tool_calls":[],"tool_results":[],"host_tasks":[],"source_id":"worker-round-2"}),
        ])),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store).await;
    let h = headers("internal-tool-loop");
    let mut req = body(json!([{"role":"user","content":"Search for weather tools."}]));
    req["tools"] = harness_tools();
    let sse = String::from_utf8(bridge.handle("actor-search", &h, &req).await.unwrap()).unwrap();
    assert!(sse.contains("Search complete."));
    assert!(!sse.contains("sic_tool_search"));
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == "worker.run")
            .count(),
        2
    );
}

#[tokio::test]
async fn status_projects_last_completed_turn_without_exposing_content() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let search_call = json!({"id":"search-1","type":"function","function":{
        "name":"sic_tool_search","arguments":"{\"query\":\"PRIVATE_ARGUMENT_SENTINEL\"}"
    }});
    let runtime = Arc::new(MockRuntime {
        worker_script: Mutex::new(VecDeque::from([
            json!({"content":null,"attempt_id":"attempt-1","tool_calls":[search_call],
                "tool_results":[{"tool_call_id":"search-1","result":{"text":"PRIVATE_RESULT_SENTINEL"}}],
                "host_tasks":[],"source_id":"worker-1"}),
            json!({"content":"PRIVATE_ANSWER_SENTINEL","attempt_id":"attempt-2",
                "tool_calls":[],"tool_results":[],"host_tasks":[],"source_id":"worker-2"}),
        ])),
        ..Default::default()
    });
    let bridge = make_bridge(runtime, store).await;
    let request = body(json!([{"role":"user","content":"PRIVATE_INPUT_SENTINEL"}]));
    bridge
        .handle("actor-a", &headers("observable"), &request)
        .await
        .unwrap();

    let status = bridge
        .session_status("actor-a", "observable")
        .await
        .unwrap();
    assert_eq!(status["active"], false);
    assert!(status["state"].is_null()); // Existing active-only fields retain their meaning.
    assert_eq!(status["last_turn"]["state"], "delivered");
    assert_eq!(status["last_turn"]["phase"], "delivered");
    assert_eq!(status["last_turn"]["worker_rounds_recorded"], 2);
    assert_eq!(status["last_turn"]["tool_completions_recorded"], 0);
    assert_eq!(status["last_turn"]["references"]["wb_id"], "wb-1");
    assert_eq!(
        status["last_turn"]["references"]["wb_source_id"],
        "source-1"
    );
    assert_eq!(status["last_turn"]["references"]["editor_job_id"], "job-1");
    assert_eq!(
        status["last_turn"]["references"]["published_cut_id"],
        "cut-1"
    );
    assert_eq!(status["last_turn"]["references"]["published_cb_id"], "cb-1");
    assert_eq!(
        status["last_turn"]["references"]["last_worker_attempt_id"],
        "attempt-2"
    );
    assert_eq!(status["issued_host_tasks"], 0);
    assert_eq!(status["revision"].as_str().unwrap().len(), 64);
    assert_eq!(
        bridge
            .session_status("actor-a", "observable")
            .await
            .unwrap()["revision"],
        status["revision"]
    );
    let serialized = status.to_string();
    for secret in [
        "PRIVATE_ARGUMENT_SENTINEL",
        "PRIVATE_RESULT_SENTINEL",
        "PRIVATE_ANSWER_SENTINEL",
        "PRIVATE_INPUT_SENTINEL",
    ] {
        assert!(!serialized.contains(secret));
    }
    let denied = bridge
        .session_status("actor-b", "observable")
        .await
        .unwrap_err();
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_keeps_published_cb_wb_references_after_worker_interruption() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime {
        pause_on: Some("worker.run".into()),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store.clone()).await;
    let task_bridge = bridge.clone();
    let task = tokio::spawn(async move {
        task_bridge
            .handle(
                "owner",
                &headers("interrupted-status"),
                &body(json!([{"role":"user","content":"fixture"}])),
            )
            .await
    });
    runtime.started.notified().await;
    let live = bridge
        .session_status("owner", "interrupted-status")
        .await
        .unwrap();
    assert_eq!(live["last_turn"]["state"], "running");
    assert_eq!(live["last_turn"]["references"]["wb_id"], "wb-1");
    assert_eq!(live["last_turn"]["references"]["published_cb_id"], "cb-1");
    assert_eq!(live["last_turn"]["references"]["published_cut_id"], "cut-1");
    task.abort();
    let _ = task.await;
    drop(bridge);
    let recovered = make_bridge(runtime, store).await;
    let status = recovered
        .session_status("owner", "interrupted-status")
        .await
        .unwrap();
    assert_eq!(status["last_turn"]["state"], "indeterminate");
    assert_eq!(status["last_turn"]["references"]["wb_id"], "wb-1");
    assert_eq!(status["last_turn"]["references"]["published_cb_id"], "cb-1");
    assert_ne!(live["revision"], status["revision"]);
}

#[tokio::test]
async fn status_http_is_actor_scoped_conditional_and_never_cached() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let bridge = make_bridge(Arc::new(MockRuntime::default()), store).await;
    let first = body(json!([{"role":"user","content":"status fixture"}]));
    bridge
        .handle("owner", &headers("status-session"), &first)
        .await
        .unwrap();
    let resolver: dsh_bridge::ActorResolver = Arc::new(|headers| {
        match headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
        {
            Some("Bearer owner") => Ok("owner".to_owned()),
            Some("Bearer other") => Ok("other".to_owned()),
            _ => Err("unauthorized".to_owned()),
        }
    });
    let app = axum::Router::new().nest("/v1", dsh_bridge::router(bridge.clone(), resolver));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://{address}/v1/sic/status");
    let anonymous = client
        .post(&url)
        .json(&json!({"session_id":"status-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.headers()["cache-control"], "no-store");
    let other = client
        .post(&url)
        .bearer_auth("other")
        .json(&json!({"session_id":"status-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    let response = client
        .post(&url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let status: Value = response.json().await.unwrap();
    let revision = status["revision"].as_str().unwrap();
    let unchanged: Value = client
        .post(&url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session","if_revision":revision}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged, json!({"unchanged":true,"revision":revision}));
    let invalid = client
        .post(&url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session","if_revision":"invalid"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let oversized = client
        .post(&url)
        .bearer_auth("owner")
        .body(vec![b' '; 4097])
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let second = body(json!([
        {"role":"user","content":"status fixture"},
        {"role":"assistant","content":"mock answer 1"},
        {"role":"user","content":"next"}
    ]));
    bridge
        .handle("owner", &headers("status-session"), &second)
        .await
        .unwrap();
    let updated: Value = client
        .post(&url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session","if_revision":revision}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(updated["revision"], revision);
    assert_eq!(updated["last_turn"]["references"]["wb_id"], "wb-2");
    let accounting_url = format!("http://{address}/v1/sic/accounting");
    let other = client
        .post(&accounting_url)
        .bearer_auth("other")
        .json(&json!({"session_id":"status-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    let forged_chain = client
        .post(&accounting_url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session","chain_id":"other-chain"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged_chain.status(), StatusCode::BAD_REQUEST);
    let accounting = client
        .post(&accounting_url)
        .bearer_auth("owner")
        .json(&json!({"session_id":"status-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(accounting.status(), StatusCode::OK);
    assert_eq!(accounting.headers()["cache-control"], "no-store");
    let accounting: Value = accounting.json().await.unwrap();
    assert_eq!(accounting["groups"][0]["role"], "editor");
    assert_eq!(accounting["unresolved_attempts"], 1);
    assert_eq!(accounting["pending_worker_actions"], 1);
    assert_eq!(accounting["superseded_worker_actions"], 1);
    assert!(accounting.get("attempts").is_none());
    server.abort();
}

#[tokio::test]
async fn axum_route_uses_host_actor_resolver_and_returns_event_stream() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime, store).await;
    let resolver: dsh_bridge::ActorResolver = Arc::new(|headers| {
        (headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer host-test"))
        .then(|| "host-actor".to_owned())
        .ok_or_else(|| "unauthorized".to_owned())
    });
    let app = axum::Router::new().nest("/v1", dsh_bridge::router(bridge, resolver));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}/v1/chat/completions"))
        .bearer_auth("host-test")
        .header("x-sic-bridge-session", "http-session")
        .json(&body(json!([{"role":"user","content":"through HTTP"}])))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    let cache_control = response.headers().get("cache-control").cloned();
    let text = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(content_type.unwrap(), "text/event-stream; charset=utf-8");
    assert_eq!(cache_control.unwrap(), "no-store");
    assert!(text.contains("mock answer 1"));
    assert!(text.ends_with("data: [DONE]\n\n"));
    server.abort();
}

#[tokio::test]
async fn restart_marks_inflight_turn_indeterminate_and_does_not_repeat_core_operation() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime {
        pause_on: Some("source.ingest".into()),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store.clone()).await;
    let h = headers("crash-test");
    let req = body(json!([{"role":"user","content":"once only"}]));
    let task_bridge = bridge.clone();
    let task_headers = h.clone();
    let task_req = req.clone();
    let task =
        tokio::spawn(async move { task_bridge.handle("actor", &task_headers, &task_req).await });
    runtime.started.notified().await;

    let second = DshBridge::new(
        store.clone(),
        runtime.clone(),
        DshBridgeConfig {
            model_alias: "sic-worker".into(),
            fixed_max_tokens: Some(4096),
            max_worker_calls_per_turn: 8,
            max_host_tasks_per_response: 4,
            max_harness_tools: 64,
            fixed_reasoning_effort: None,
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(second.code, "bridge_instance_active");
    let live_status = bridge.session_status("actor", "crash-test").await.unwrap();
    assert_eq!(live_status["state"], "running");
    assert_eq!(live_status["active"], true);

    task.abort();
    let _ = task.await;
    drop(bridge);

    let recovered = make_bridge(runtime.clone(), store).await;
    let error = recovered.handle("actor", &h, &req).await.unwrap_err();
    assert_eq!(error.code, "turn_indeterminate");
    assert_eq!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>(),
        vec!["chain.create", "source.ingest"]
    );
    let status = recovered
        .session_status("actor", "crash-test")
        .await
        .unwrap();
    assert_eq!(status["state"], "indeterminate");
    assert_eq!(status["phase"], "source_ingest_started");
}

#[tokio::test]
async fn file_store_bridge_guard_excludes_independently_opened_store() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bridge-guard.sqlite");
    let store1 = sic_core_rust::storage::Store::create(&path).await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge1 = make_bridge(runtime.clone(), store1).await;

    let store2 = sic_core_rust::storage::Store::open(&path).await.unwrap();
    let rejected = DshBridge::new(
        store2.clone(),
        runtime.clone(),
        DshBridgeConfig {
            model_alias: "sic-worker".into(),
            fixed_max_tokens: Some(4096),
            max_worker_calls_per_turn: 8,
            max_host_tasks_per_response: 4,
            max_harness_tools: 64,
            fixed_reasoning_effort: None,
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(rejected.code, "bridge_instance_active");

    drop(bridge1);
    let reopened = DshBridge::new(
        store2,
        runtime,
        DshBridgeConfig {
            model_alias: "sic-worker".into(),
            fixed_max_tokens: Some(4096),
            max_worker_calls_per_turn: 8,
            max_host_tasks_per_response: 4,
            max_harness_tools: 64,
            fixed_reasoning_effort: None,
        },
    )
    .await
    .unwrap();
    drop(reopened);
}

#[tokio::test]
async fn bridge_accepts_129_discoverable_harness_tools() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = Arc::new(
        DshBridge::new(
            store,
            runtime.clone(),
            DshBridgeConfig {
                model_alias: "sic-worker".into(),
                fixed_max_tokens: Some(4096),
                max_worker_calls_per_turn: 8,
                max_host_tasks_per_response: 4,
                max_harness_tools: 4096,
                fixed_reasoning_effort: None,
            },
        )
        .await
        .unwrap(),
    );
    let tools = (0..129)
        .map(|index| {
            json!({
                "type":"function",
                "function":{
                    "name":format!("host_tool_{index}"),
                    "description":"host tool",
                    "parameters":{"type":"object","properties":{},"additionalProperties":false}
                }
            })
        })
        .collect::<Vec<_>>();
    let mut req = body(json!([{"role":"user","content":"continue"}]));
    req["tools"] = json!(tools);

    bridge
        .handle("actor-wide-catalog", &headers("wide-catalog"), &req)
        .await
        .unwrap();
    let calls = runtime.calls.lock().unwrap();
    let configured = calls
        .iter()
        .find(|call| call.0 == "tools.configure")
        .expect("host catalog must reach the runtime");
    assert_eq!(configured.2["tools"].as_array().unwrap().len(), 129);
    assert_eq!(
        configured.2["acl"]["discover"].as_array().unwrap().len(),
        129
    );
}

#[tokio::test]
async fn failed_turn_fingerprint_is_reclaimed_on_next_session_turn() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime::default());
    let bridge = make_bridge(runtime, store.clone()).await;
    let mut tx = store.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO dsh_bridge_sessions(actor,session_key,model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count) VALUES(?,?,?,?,?,?,?,?,0,0)",
    )
    .bind("actor-a")
    .bind("failed-session")
    .bind("sic-worker")
    .bind("chain-test")
    .bind("[]")
    .bind("[]")
    .bind("failed-fp")
    .bind("")
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO dsh_bridge_turns(actor,session_key,fingerprint,state,phase,progress,response_sse,error_code,created) VALUES(?,?,?,'failed','provider','{}',NULL,'provider_error',1)",
    )
    .bind("actor-a")
    .bind("failed-session")
    .bind("failed-fp")
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let response = bridge
        .handle(
            "actor-a",
            &headers("failed-session"),
            &body(json!([
                {"role":"system","content":"preserve facts"},
                {"role":"user","content":"retry after failed turn"}
            ])),
        )
        .await
        .unwrap();
    let text = String::from_utf8(response).unwrap();
    assert!(text.contains("mock answer"), "{text}");
}

// A provider failure after edit.run is intentionally indeterminate: a direct
// retry must stay busy until the host retires the unresolved turn. The
// definite-failure recovery path is covered by the preseeded failed-turn test
// above; do not conflate the two.
#[tokio::test]
async fn indeterminate_edit_run_failure_requires_session_retirement() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime {
        fail_edit_run_once: AtomicBool::new(true),
        ..Default::default()
    });
    let bridge = make_bridge(runtime, store).await;
    let h = headers("failed-edit-session");

    let first = body(json!([
        {"role":"system","content":"preserve facts"},
        {"role":"user","content":"first request"}
    ]));
    let error = bridge.handle("actor-a", &h, &first).await.unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_GATEWAY);

    let second = body(json!([
        {"role":"system","content":"preserve facts"},
        {"role":"user","content":"first request"},
        {"role":"user","content":"retry after provider failure"}
    ]));
    let busy = bridge.handle("actor-a", &h, &second).await.unwrap_err();
    assert_eq!(busy.status, StatusCode::CONFLICT);
    assert_eq!(busy.code, "session_busy");
}

#[tokio::test]
async fn stale_pending_source_is_filtered_before_wb_form() {
    let store = sic_core_rust::storage::Store::memory().await.unwrap();
    let runtime = Arc::new(MockRuntime {
        chain_pending: Some(json!({
            "cut_seq":0,
            "contexts":{"main":{"levels":{"1":{"pending":["source-1"]}}}},
            "groups":{"source-1":[]}
        })),
        ..Default::default()
    });
    let bridge = make_bridge(runtime.clone(), store.clone()).await;
    let mut tx = store.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO dsh_bridge_sessions(actor,session_key,model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count) VALUES(?,?,?,?,?,?,NULL,?,0,0)",
    )
    .bind("actor-a")
    .bind("stale-source-session")
    .bind("sic-worker")
    .bind("chain-test")
    .bind("[]")
    .bind(r#"["stale-source"]"#)
    .bind("")
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    bridge
        .handle(
            "actor-a",
            &headers("stale-source-session"),
            &body(json!([
                {"role":"system","content":"preserve facts"},
                {"role":"user","content":"new material"}
            ])),
        )
        .await
        .unwrap();
    let calls = runtime.calls.lock().unwrap();
    let wb = calls
        .iter()
        .find(|call| call.0 == "wb.form")
        .expect("wb.form");
    assert_eq!(wb.2["source_ids"], json!(["source-1"]));
}
