use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    dsh_bridge::{CoreRuntime, DshBridge, DshBridgeConfig},
    storage::{self, Store},
};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Fixture(Arc<Mutex<Vec<Value>>>);

async fn completion(State(fixture): State<Fixture>, Json(request): Json<Value>) -> Json<Value> {
    let mut requests = fixture.0.lock().await;
    requests.push(request);
    let index = requests.len() - 1;
    let (finish, message) = match index {
        0 | 2 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("editor-{index}"),"type":"function","function":{
                "name":"sic_edit_blocks","arguments":json!({"ops":if index == 0 {json!([])} else {json!([{"op":"DELETE","target":"u1"}])},"pvi":if index == 0 {json!({"V":["volatile synthetic fact"]})} else {json!({})}}).to_string()
            }}]}),
        ),
        _ => (
            "stop",
            json!({"role":"assistant","content":format!("worker answer {index}")}),
        ),
    };
    Json(json!({"model":"mock-model","choices":[{"finish_reason":finish,"message":message}]}))
}

fn request(messages: &[Value]) -> Value {
    json!({"model":"sic-worker","messages":messages,"stream":true,"tools":[],"tool_choice":"auto","max_tokens":4096})
}

async fn exercise_volatile(restart: bool, legacy_preformed: bool) {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"LOCAL_FIXTURE",
        "tokenizer_model":"host-o200k","context_window":1000000,"max_input_tokens":1000000,"max_output_tokens":393216,"default_output_tokens":4096,"timeout_seconds":10});
    let directory = tempdir().unwrap();
    let database = directory.path().join("volatile.sqlite");
    let mut store = Store::create(&database).await.unwrap();
    let config = json!({"roles":{"worker":profile,"editor":profile}});
    let mut core = Core::with_model_config_and_resolver(
        store.clone(),
        config.clone(),
        Arc::new(|_| Ok("synthetic-key".into())),
    )
    .unwrap();
    let bridge_config = DshBridgeConfig {
        model_alias: "sic-worker".into(),
        fixed_max_tokens: Some(4096),
        max_worker_calls_per_turn: 8,
        max_host_tasks_per_response: 4,
        max_harness_tools: 64,
        fixed_reasoning_effort: None,
    };
    let mut bridge = DshBridge::new(
        store.clone(),
        Arc::new(CoreRuntime::new(core.clone())),
        bridge_config.clone(),
    )
    .await
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-sic-bridge-session", "volatile-session".parse().unwrap());
    let mut history = vec![json!({"role":"user","content":"first input"})];
    bridge
        .handle("alice", &headers, &request(&history))
        .await
        .unwrap();
    assert!(
        fixture.0.lock().await[1]
            .to_string()
            .contains("volatile synthetic fact")
    );
    let mut original_legacy_wb = None;
    if legacy_preformed {
        let status = bridge
            .session_status("alice", "volatile-session")
            .await
            .unwrap();
        let chain = status["chain_id"].as_str().unwrap();
        let mut tx = store.begin().await.unwrap();
        let mut state = storage::get_chain(&mut tx, chain, "alice").await.unwrap();
        let level = &mut state["contexts"]["main"]["levels"]["1"];
        let volatile = level["pending"][0].as_str().unwrap().to_owned();
        let old_wb = json!({"wb_id":"legacy-volatile-wb","chain_id":chain,"context_id":"main","level":1,
            "sequence":2,"source_id":volatile,"source_ids":[volatile],"unit_ids":["u1"],"members":[],
            "reason":"volatile-continuation","formed_at":1});
        storage::put_object(&mut tx, "wb", "legacy-volatile-wb", &old_wb)
            .await
            .unwrap();
        level["wbs"]
            .as_array_mut()
            .unwrap()
            .push(json!("legacy-volatile-wb"));
        // Reproduce the old Rust serialization shape and its bridge's failure
        // to record the volatile root in pending_sources.
        level.as_object_mut().unwrap().remove("superseded_wbs");
        storage::save_chain(&mut tx, &state).await.unwrap();
        let pending_text: String = sqlx::query_scalar(
            "SELECT pending_sources FROM dsh_bridge_sessions WHERE actor=? AND session_key=?",
        )
        .bind("alice")
        .bind("volatile-session")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let mut saved_pending: Vec<String> = serde_json::from_str(&pending_text).unwrap();
        assert_eq!(saved_pending.len(), 2);
        saved_pending.retain(|source| source != &volatile);
        sqlx::query(
            "UPDATE dsh_bridge_sessions SET pending_sources=? WHERE actor=? AND session_key=?",
        )
        .bind(serde_json::to_string(&saved_pending).unwrap())
        .bind("alice")
        .bind("volatile-session")
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        original_legacy_wb = Some(old_wb);
    }
    if restart {
        drop(bridge);
        drop(core);
        store.close().await;
        store = Store::open(&database).await.unwrap();
        core = Core::with_model_config_and_resolver(
            store.clone(),
            config,
            Arc::new(|_| Ok("synthetic-key".into())),
        )
        .unwrap();
        bridge = DshBridge::new(
            store.clone(),
            Arc::new(CoreRuntime::new(core.clone())),
            bridge_config,
        )
        .await
        .unwrap();
    }
    history.push(json!({"role":"assistant","content":"worker answer 1"}));
    history.push(json!({"role":"user","content":"second input"}));
    bridge
        .handle("alice", &headers, &request(&history))
        .await
        .unwrap();
    let requests = fixture.0.lock().await;
    let second_editor = requests[2].to_string();
    assert!(second_editor.contains("volatile synthetic fact"));
    assert!(second_editor.contains("worker answer 1"));
    assert!(second_editor.contains("second input"));
    drop(requests);
    let status = bridge
        .session_status("alice", "volatile-session")
        .await
        .unwrap();
    let chain = status["chain_id"].as_str().unwrap();
    let context = OperationContext {
        actor: "alice".into(),
        chain_id: Some(chain.into()),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    let cut = core.operate("cut.get", &context, &json!({})).await.unwrap();
    let cb = &cut["result"]["view"]["cbs"][1];
    assert!(
        !cb["body"]
            .as_str()
            .unwrap()
            .contains("volatile synthetic fact")
    );
    assert!(cb["body"].as_str().unwrap().contains("worker answer 1"));
    assert!(cb["body"].as_str().unwrap().contains("second input"));
    assert!(
        cb["ops"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op["op"] == "DELETE" && op["target"] == "u1")
    );
    let expanded = core
        .operate("hierarchy.expand", &context, &json!({"cb_id":cb["cb_id"]}))
        .await
        .unwrap();
    assert_eq!(
        expanded["result"]["source_ids"].as_array().unwrap().len(),
        3
    );
    if let Some(old_wb) = original_legacy_wb {
        let timeline = core
            .operate(
                "hierarchy.timeline",
                &context,
                &json!({"level":1,"kind":"wb"}),
            )
            .await
            .unwrap();
        let rows = timeline["result"]["rows"].as_array().unwrap();
        let old_row = rows
            .iter()
            .find(|row| row["wb_id"] == "legacy-volatile-wb")
            .unwrap();
        assert!(old_row["cb_id"].is_null());
        assert_eq!(old_row["superseded_by"], expanded["result"]["wb_id"]);
        let mut tx = store.begin().await.unwrap();
        assert_eq!(
            storage::get_object(&mut tx, "legacy-volatile-wb", Some("wb"))
                .await
                .unwrap(),
            old_wb
        );
        tx.commit().await.unwrap();
        let old_expanded = core
            .operate(
                "hierarchy.expand",
                &context,
                &json!({"wb_id":"legacy-volatile-wb"}),
            )
            .await
            .unwrap();
        assert_eq!(
            old_expanded["result"]["original_units"][0]["text"],
            "volatile synthetic fact"
        );
        let replacement = expanded["result"]["wb_id"].clone();
        let cb_id = cb["cb_id"].clone();
        drop(bridge);
        drop(core);
        store.close().await;
        let reopened = Store::open(&database).await.unwrap();
        let reopened_core = Core::new(reopened.clone());
        let after_restart = reopened_core
            .operate(
                "hierarchy.timeline",
                &context,
                &json!({"level":1,"kind":"wb"}),
            )
            .await
            .unwrap();
        let rows = after_restart["result"]["rows"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row["wb_id"] == "legacy-volatile-wb")
                .unwrap()["superseded_by"],
            replacement
        );
        assert_eq!(
            rows.iter().find(|row| row["wb_id"] == replacement).unwrap()["cb_id"],
            cb_id
        );
        let mut tx = reopened.begin().await.unwrap();
        assert_eq!(
            storage::get_object(&mut tx, "legacy-volatile-wb", Some("wb"))
                .await
                .unwrap(),
            old_wb
        );
        tx.commit().await.unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn volatile_content_reaches_worker_and_the_next_combined_wb() {
    exercise_volatile(false, false).await;
}

#[tokio::test]
async fn volatile_content_survives_bridge_and_database_restart() {
    exercise_volatile(true, false).await;
}

#[tokio::test]
async fn old_preformed_volatile_wb_is_preserved_and_superseded_without_losing_session() {
    exercise_volatile(true, true).await;
}

#[tokio::test]
async fn earlier_pending_cannot_supersede_a_wb_with_an_existing_editor_job() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut ctx = OperationContext {
        actor: "alice".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    ctx.chain_id = Some(
        core.operate("chain.create", &ctx, &json!({}))
            .await
            .unwrap()["result"]["chain_id"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let first = core
        .operate(
            "source.ingest",
            &ctx,
            &json!({"request":{"messages":[{"role":"user","content":"V"}]}}),
        )
        .await
        .unwrap();
    core.operate(
        "wb.form",
        &ctx,
        &json!({"source_ids":[first["result"]["source_id"]],"reason":"volatile-continuation"}),
    )
    .await
    .unwrap();
    let later = core
        .operate(
            "source.ingest",
            &ctx,
            &json!({"request":{"messages":[{"role":"user","content":"later"}]}}),
        )
        .await
        .unwrap();
    core.operate(
        "edit.prepare",
        &ctx,
        &json!({"source_id":first["result"]["source_id"]}),
    )
    .await
    .unwrap();
    let before = core.operate("chain.get", &ctx, &json!({})).await.unwrap()["result"].clone();
    let result = core
        .operate(
            "wb.form",
            &ctx,
            &json!({"source_ids":[later["result"]["source_id"]],"include_earlier_pending":true}),
        )
        .await;
    assert_eq!(result.unwrap_err().code, "conflict");
    let after = core.operate("chain.get", &ctx, &json!({})).await.unwrap()["result"].clone();
    assert_eq!(after["groups"], before["groups"]);
    assert_eq!(after["contexts"], before["contexts"]);
    assert_eq!(after["jobs"], before["jobs"]);
}

async fn host_tool_completion(
    State(fixture): State<Fixture>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let mut requests = fixture.0.lock().await;
    requests.push(request);
    let index = requests.len() - 1;
    let (finish, message) = match index {
        0 | 3 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{
            "id":format!("editor-{index}"),"type":"function","function":{"name":"sic_edit_blocks",
                "arguments":json!({"ops":if index == 0 {json!([])} else {json!([{"op":"DELETE","target":"u1"}])},
                    "pvi":if index == 0 {json!({"V":["volatile across tool"]})} else {json!({})}}).to_string()}}]}),
        ),
        1 => (
            "tool_calls",
            json!({"role":"assistant","content":null,"tool_calls":[{
            "id":"host-call","type":"function","function":{"name":"sic_tool_call",
                "arguments":json!({"name":"fixture","arguments":{"x":1}}).to_string()}}]}),
        ),
        2 => ("stop", json!({"role":"assistant","content":"after tool"})),
        _ => (
            "stop",
            json!({"role":"assistant","content":"after next turn"}),
        ),
    };
    Json(json!({"model":"mock-model","choices":[{"finish_reason":finish,"message":message}]}))
}

#[tokio::test]
async fn volatile_survives_external_tool_continuation_and_restart_without_tool_replay() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(host_tool_completion))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/mock-model","api_base":format!("http://{addr}/v1"),"api_key_env":"LOCAL_FIXTURE",
        "tokenizer_model":"host-o200k","context_window":1000000,"max_input_tokens":1000000,"max_output_tokens":393216,"default_output_tokens":4096,"timeout_seconds":10});
    let config = json!({"roles":{"worker":profile,"editor":profile}});
    let directory = tempdir().unwrap();
    let database = directory.path().join("tool-volatile.sqlite");
    let store = Store::create(&database).await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        config.clone(),
        Arc::new(|_| Ok("synthetic-key".into())),
    )
    .unwrap();
    let bridge_config = DshBridgeConfig {
        model_alias: "sic-worker".into(),
        fixed_max_tokens: Some(4096),
        max_worker_calls_per_turn: 8,
        max_host_tasks_per_response: 4,
        max_harness_tools: 64,
        fixed_reasoning_effort: None,
    };
    let bridge = DshBridge::new(
        store.clone(),
        Arc::new(CoreRuntime::new(core.clone())),
        bridge_config.clone(),
    )
    .await
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-sic-bridge-session",
        "volatile-tool-session".parse().unwrap(),
    );
    let tool_schema = json!([{"type":"function","function":{"name":"fixture","description":"synthetic fixture",
        "parameters":{"type":"object","properties":{"x":{"type":"integer"}}}}}]);
    let mut history = vec![json!({"role":"user","content":"first input"})];
    let mut initial = request(&history);
    initial["tools"] = tool_schema.clone();
    let response = bridge.handle("alice", &headers, &initial).await.unwrap();
    assert!(String::from_utf8(response).unwrap().contains("host-call"));
    let status = bridge
        .session_status("alice", "volatile-tool-session")
        .await
        .unwrap();
    let chain = status["chain_id"].as_str().unwrap().to_owned();
    drop(bridge);
    drop(core);
    store.close().await;
    let store = Store::open(&database).await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        config,
        Arc::new(|_| Ok("synthetic-key".into())),
    )
    .unwrap();
    let bridge = DshBridge::new(
        store,
        Arc::new(CoreRuntime::new(core.clone())),
        bridge_config,
    )
    .await
    .unwrap();
    history.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":"host-call","type":"function","function":{"name":"fixture","arguments":"{\"x\":1}"}}]}));
    let long_result = format!(
        "tool result marker{}END_OF_LONG_TOOL_RESULT",
        "x".repeat(50_000)
    );
    history.push(
        json!({"role":"tool","tool_call_id":"host-call","name":"fixture","content":long_result}),
    );
    let mut continuation = request(&history);
    continuation["tools"] = tool_schema.clone();
    let response = bridge
        .handle("alice", &headers, &continuation)
        .await
        .unwrap();
    assert!(String::from_utf8(response).unwrap().contains("after tool"));
    history.push(json!({"role":"assistant","content":"after tool"}));
    history.push(json!({"role":"user","content":"next user input"}));
    let mut next = request(&history);
    next["tools"] = tool_schema;
    bridge.handle("alice", &headers, &next).await.unwrap();
    let requests = fixture.0.lock().await;
    assert!(requests[2].to_string().contains("volatile across tool"));
    assert!(requests[2].to_string().contains("tool result marker"));
    assert!(!requests[2].to_string().contains("END_OF_LONG_TOOL_RESULT"));
    let native_tool = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "host-call")
        .unwrap();
    let projected: Value = serde_json::from_str(native_tool["content"].as_str().unwrap()).unwrap();
    let projection = &projected["sic_tool_result_preview"];
    assert_eq!(projection["preview_chars"], 32_768);
    let archived_source = projection["source_id"].clone();
    let archived_unit = projection["unit_id"].clone();
    let total = projection["total_chars"].as_u64().unwrap();
    assert!(requests[3].to_string().contains("volatile across tool"));
    assert!(requests[3].to_string().contains("tool result marker"));
    assert!(requests[3].to_string().contains("END_OF_LONG_TOOL_RESULT"));
    assert!(requests[3].to_string().contains("next user input"));
    drop(requests);
    let context = OperationContext {
        actor: "alice".into(),
        chain_id: Some(chain),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    let tasks = core
        .operate("tools.tasks", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(tasks["result"]["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(
        tasks["result"]["tasks"][0]["status"],
        "delivered_unattested"
    );
    let tail = core
        .operate(
            "source.text",
            &context,
            &json!({"source_id":archived_source,"unit_id":archived_unit,
        "offset":total-64,"limit":64}),
        )
        .await
        .unwrap();
    assert!(
        tail["result"]["text"]
            .as_str()
            .unwrap()
            .contains("END_OF_LONG_TOOL_RESULT")
    );
    let cut = core.operate("cut.get", &context, &json!({})).await.unwrap();
    let second_cb = &cut["result"]["view"]["cbs"][1];
    assert!(
        !second_cb["body"]
            .as_str()
            .unwrap()
            .contains("volatile across tool")
    );
    assert!(
        second_cb["body"]
            .as_str()
            .unwrap()
            .contains("tool result marker")
    );
    server.abort();
}
