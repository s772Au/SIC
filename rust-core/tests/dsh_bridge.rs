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
    sync::{Arc, Mutex},
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[derive(Default)]
struct MockRuntime {
    calls: Mutex<Vec<(String, Option<String>, Value, bool)>>,
    pause_on: Option<String>,
    fail_claim_task: Option<String>,
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
        let wb_source_id = payload
            .get("source_ids")
            .and_then(Value::as_array)
            .and_then(|ids| ids.first())
            .cloned()
            .unwrap_or(Value::Null);
        let claim_task_id = payload
            .get("task_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let fail_claim = operation == "tool.claim"
            && self.fail_claim_task.as_deref() == claim_task_id.as_deref();
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
                "tool.complete" => json!({"task_id":"task-1","status":"delivered_unattested","succeeded":null,"source_id":"worker-1","open_calls":[]}),
                "source.ingest" => json!({"source_id":format!("source-{n}")}),
                "wb.form" => json!({"wb_id":format!("wb-{n}"),"source_id":wb_source_id}),
                "edit.prepare" => json!({"job_id":format!("job-{n}")}),
                "edit.run" => json!({"status":"ready"}),
                "chain.get" => json!({"cut_seq":n - 1}),
                "cut.publish" => json!({"cut_id":format!("cut-{n}")}),
                "worker.run" => scripted.unwrap_or_else(|| json!({"content":format!("mock answer {n}"),"source_id":format!("worker-{n}"),"tool_calls":[],"tool_results":[],"host_tasks":[]})),
                _ => return Err("unsupported mock operation".to_owned()),
            };
            Ok(
                json!({"ok":true,"operation":operation,"result":result,"evidence":{"replayed":replayed}}),
            )
        })
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
