use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    dsh_bridge::{BoxFuture, CoreRuntime, DshBridge, DshBridgeConfig, DshRuntime},
    storage::Store,
};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{path::Path, sync::Arc};
use tempfile::tempdir;
use tokio::sync::Mutex;

const OWNER: &str = "multiturn-owner";
const SESSION: &str = "multiturn-session";
const TURN_COUNT: usize = 24;
const RESTART_AFTER: usize = 12;

#[derive(Clone, Default)]
struct ProviderFixture {
    requests: Arc<Mutex<Vec<Value>>>,
    core_errors: Arc<Mutex<Vec<Value>>>,
}

async fn completion(
    State(fixture): State<ProviderFixture>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let index = {
        let mut calls = fixture.requests.lock().await;
        calls.push(request);
        calls.len() - 1
    };
    let (finish_reason, message) = if index % 2 == 0 {
        let turn = index / 2 + 1;
        (
            "tool_calls",
            json!({
                "role":"assistant",
                "content":null,
                "tool_calls":[{
                    "id":format!("editor-call-{turn}"),
                    "type":"function",
                    "function":{"name":"sic_edit_blocks","arguments":json!({
                        "ops":[],"pvi":{"P":[format!("immutable synthetic fact {turn}")]}
                    }).to_string()}
                }]
            }),
        )
    } else {
        let turn = index / 2 + 1;
        (
            "stop",
            json!({"role":"assistant","content":format!("mock worker answer {turn}")}),
        )
    };
    (
        StatusCode::OK,
        Json(json!({
            "model":"deepseek-v4.1-flash-eco",
            "usage":{"prompt_tokens":15,"completion_tokens":6},
            "choices":[{"finish_reason":finish_reason,"message":message}]
        })),
    )
}

fn headers(session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-sic-bridge-session",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

fn request(messages: &[Value]) -> Value {
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

fn provider_config(endpoint: String) -> Value {
    let profile = json!({
        "model":"openai/deepseek-v4.1-flash-eco",
        "api_base":endpoint,
        "api_key_env":"LOCAL_MULTITURN_FIXTURE",
        "tokenizer_model":"host-o200k-surrogate",
        "tokenizer_status":"surrogate-not-provider-native",
        "context_window":1_000_000,
        "max_input_tokens":1_000_000,
        "max_output_tokens":393_216,
        "default_output_tokens":4096,
        "timeout_seconds":10
    });
    json!({"roles":{"worker":profile,"editor":profile}})
}

fn bridge_config() -> DshBridgeConfig {
    DshBridgeConfig {
        model_alias: "sic-worker".into(),
        fixed_max_tokens: Some(4096),
        max_worker_calls_per_turn: 8,
        max_host_tasks_per_response: 4,
        max_harness_tools: 64,
        fixed_reasoning_effort: None,
    }
}

async fn start_provider(fixture: ProviderFixture) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(fixture),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}/v1"), server)
}

async fn new_core(store: Store, config: Value) -> Core {
    Core::with_model_config_and_resolver(
        store,
        config,
        Arc::new(|_| Ok("synthetic-only-key".into())),
    )
    .unwrap()
}

#[derive(Clone)]
struct TracedCoreRuntime {
    inner: CoreRuntime,
    errors: Arc<Mutex<Vec<Value>>>,
}

impl DshRuntime for TracedCoreRuntime {
    fn operate(
        &self,
        actor: String,
        chain_id: Option<String>,
        operation: String,
        payload: Value,
        allow_model: bool,
    ) -> BoxFuture<Result<Value, String>> {
        let inner = self.inner.clone();
        let errors = self.errors.clone();
        Box::pin(async move {
            let result = inner
                .operate(
                    actor,
                    chain_id,
                    operation.clone(),
                    payload.clone(),
                    allow_model,
                )
                .await;
            if matches!(
                operation.as_str(),
                "source.ingest" | "wb.form" | "edit.prepare" | "worker.run"
            ) {
                let result_summary = result.as_ref().ok().map(|value| {
                    json!({
                        "source_id":value.pointer("/result/source_id"),
                        "wb_id":value.pointer("/result/wb_id"),
                        "job_id":value.pointer("/result/job_id")
                    })
                });
                errors.lock().await.push(json!({
                    "operation":operation,
                    "payload_source_id":payload.get("source_id"),
                    "payload_source_ids":payload.get("source_ids"),
                    "result":result_summary,
                    "error_code":result.as_ref().err()
                }));
            }
            result
        })
    }
}

async fn new_bridge_traced(
    store: Store,
    core: Core,
    errors: Arc<Mutex<Vec<Value>>>,
) -> Arc<DshBridge> {
    Arc::new(
        DshBridge::new(
            store,
            Arc::new(TracedCoreRuntime {
                inner: CoreRuntime::new(core),
                errors,
            }),
            bridge_config(),
        )
        .await
        .unwrap(),
    )
}

fn sse_text(bytes: &[u8]) -> String {
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let mut text = String::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(piece) = chunk
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            text.push_str(piece);
        }
    }
    text
}

async fn persisted_history(path: &Path, actor: &str, session: &str) -> Value {
    let pool: SqlitePool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    let row =
        sqlx::query("SELECT history FROM dsh_bridge_sessions WHERE actor=? AND session_key=?")
            .bind(actor)
            .bind(session)
            .fetch_one(&pool)
            .await
            .unwrap();
    let text: String = row.try_get("history").unwrap();
    pool.close().await;
    serde_json::from_str(&text).unwrap()
}

async fn cut_snapshot(core: &Core, chain_id: &str) -> (u64, Value) {
    let context = OperationContext {
        actor: OWNER.into(),
        chain_id: Some(chain_id.into()),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    let result = core.operate("cut.get", &context, &json!({})).await.unwrap();
    (
        result["result"]["sequence"].as_u64().unwrap(),
        result["result"]["view"]["cbs"].clone(),
    )
}

fn assert_combined_wb_content(cbs: &Value, turn: usize) {
    let body = cbs.as_array().unwrap().last().unwrap()["body"]
        .as_str()
        .unwrap();
    let current = format!("synthetic user turn {turn}");
    let user_position = body
        .find(&current)
        .expect("new user material must reach the published CB");
    if turn > 1 {
        let previous = format!("mock worker answer {}", turn - 1);
        let worker_position = body
            .find(&previous)
            .expect("previous Worker material must reach the published CB");
        assert!(
            worker_position < user_position,
            "merged WB must preserve Worker-before-user chronology"
        );
    }
}

async fn run_turn(
    bridge: &DshBridge,
    core: &Core,
    provider: &ProviderFixture,
    core_errors: &Arc<Mutex<Vec<Value>>>,
    messages: &mut Vec<Value>,
    turn: usize,
) -> (Vec<u8>, Value) {
    messages.push(json!({"role":"user","content":format!("synthetic user turn {turn}")}));
    let body = request(messages);
    let response = bridge.handle(OWNER, &headers(SESSION), &body).await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let status = bridge.session_status(OWNER, SESSION).await.ok();
            let chain_summary = if let Some(chain_id) = status
                .as_ref()
                .and_then(|status| status["chain_id"].as_str())
            {
                let context = OperationContext {
                    actor: OWNER.into(),
                    chain_id: Some(chain_id.into()),
                    allow_write: false,
                    allow_model: false,
                    allow_revoke: false,
                };
                let chain_result = core.operate("chain.get", &context, &json!({})).await.ok();
                chain_result.map(|chain| {
                        let jobs = chain["result"]["jobs"].as_object().map(|jobs| {
                            jobs.iter().map(|(id, job)| json!({"id":id,"source_id":job["source_id"],"status":job["status"]})).collect::<Vec<_>>()
                        }).unwrap_or_default();
                        json!({"run_state":chain["result"]["run_state"],"cut_seq":chain["result"]["cut_seq"],
                            "worker_claim":chain["result"]["worker_claim"],
                            "pending":chain.pointer("/result/contexts/main/levels/1/pending"),"jobs":jobs})
                    })
            } else {
                None
            };
            panic!(
                "turn {turn} failed: {error:?}; status={status:?}; core={chain_summary:?}; provider_calls={}; core_errors={:?}",
                provider.requests.lock().await.len(),
                core_errors.lock().await
            );
        }
    };
    assert_eq!(sse_text(&response), format!("mock worker answer {turn}"));
    let assistant = json!({"role":"assistant","content":sse_text(&response)});
    messages.push(assistant.clone());
    (response, body)
}

#[tokio::test]
async fn twenty_four_editor_worker_turns_preserve_history_cuts_and_replay_across_restart() {
    let provider = ProviderFixture::default();
    let (endpoint, provider_server) = start_provider(provider.clone()).await;
    let model_config = provider_config(endpoint);
    let directory = tempdir().unwrap();
    let database = directory.path().join("dsh-multiturn.sqlite");
    let store = Store::create(&database).await.unwrap();
    let core = new_core(store.clone(), model_config.clone()).await;
    let bridge = new_bridge_traced(store.clone(), core.clone(), provider.core_errors.clone()).await;

    let mut messages = vec![json!({"role":"system","content":"Keep prior messages and facts."})];
    let mut previous_cbs = json!([]);
    let mut last_request = Value::Null;
    let mut last_response = Vec::new();
    let mut chain_id = String::new();

    for turn in 1..=RESTART_AFTER {
        let (response, body) = run_turn(
            &bridge,
            &core,
            &provider,
            &provider.core_errors,
            &mut messages,
            turn,
        )
        .await;
        last_request = body;
        last_response = response;
        chain_id = bridge.session_status(OWNER, SESSION).await.unwrap()["chain_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (sequence, cbs) = cut_snapshot(&core, &chain_id).await;
        assert_eq!(sequence, turn as u64, "cut sequence after turn {turn}");
        assert_eq!(cbs.as_array().unwrap().len(), turn);
        assert_combined_wb_content(&cbs, turn);
        assert_eq!(
            &cbs.as_array().unwrap()[..previous_cbs.as_array().unwrap().len()],
            previous_cbs.as_array().unwrap().as_slice(),
            "earlier CB values changed at turn {turn}"
        );
        previous_cbs = cbs;
    }
    let count_before_replay = provider.requests.lock().await.len();
    let replay = bridge
        .handle(OWNER, &headers(SESSION), &last_request)
        .await
        .unwrap();
    assert_eq!(replay, last_response);
    assert_eq!(provider.requests.lock().await.len(), count_before_replay);

    let history_before_restart = persisted_history(&database, OWNER, SESSION).await;
    assert_eq!(history_before_restart, json!(messages));
    drop(bridge);
    drop(core);
    store.close().await;

    let reopened = Store::open(&database).await.unwrap();
    let recovered_core = new_core(reopened.clone(), model_config.clone()).await;
    let recovered_bridge = new_bridge_traced(
        reopened.clone(),
        recovered_core.clone(),
        provider.core_errors.clone(),
    )
    .await;
    assert_eq!(
        persisted_history(&database, OWNER, SESSION).await,
        json!(messages),
        "restart must retain the full native DSH history"
    );
    let restart_replay = recovered_bridge
        .handle(OWNER, &headers(SESSION), &last_request)
        .await
        .unwrap();
    assert_eq!(restart_replay, last_response);
    assert_eq!(provider.requests.lock().await.len(), count_before_replay);

    for turn in (RESTART_AFTER + 1)..=TURN_COUNT {
        let (response, body) = run_turn(
            &recovered_bridge,
            &recovered_core,
            &provider,
            &provider.core_errors,
            &mut messages,
            turn,
        )
        .await;
        last_request = body;
        last_response = response;
        let (sequence, cbs) = cut_snapshot(&recovered_core, &chain_id).await;
        assert_eq!(sequence, turn as u64, "cut sequence after turn {turn}");
        assert_eq!(cbs.as_array().unwrap().len(), turn);
        assert_combined_wb_content(&cbs, turn);
        assert_eq!(
            &cbs.as_array().unwrap()[..previous_cbs.as_array().unwrap().len()],
            previous_cbs.as_array().unwrap().as_slice(),
            "earlier CB values changed at turn {turn}"
        );
        previous_cbs = cbs;
    }
    let before_final_replay = provider.requests.lock().await.len();
    assert_eq!(before_final_replay, TURN_COUNT * 2);
    let final_replay = recovered_bridge
        .handle(OWNER, &headers(SESSION), &last_request)
        .await
        .unwrap();
    assert_eq!(final_replay, last_response);
    assert_eq!(provider.requests.lock().await.len(), before_final_replay);
    assert_eq!(
        persisted_history(&database, OWNER, SESSION).await,
        json!(messages)
    );
    provider_server.abort();
}
