use axum::http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    dsh_bridge::{CoreRuntime, DshBridge, DshBridgeConfig},
    storage::Store,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn dsh_reports_prefix_conflict_without_disclosing_or_replacing_harness_instructions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let profile = json!({"model":"openai/mock-model","api_base":"http://127.0.0.1:1/v1","api_key_env":"MOCK",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"fixture","context_window":1000000,
        "max_input_tokens":1000000,"max_output_tokens":393216,"default_output_tokens":4096,"timeout_seconds":10});
    let store = Store::memory().await.unwrap();
    let core = Core::with_model_config_and_resolver(
        store.clone(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok("fixture".into())
        }),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    context.chain_id = Some(chain.clone());
    let source=core.operate("source.ingest",&context,&json!({"request":{"messages":[
        {"role":"system","content":"SYNTHETIC_OLD_PROTECTED"},{"role":"user","content":"initial task"}]}})).await.unwrap();
    let job = core
        .operate(
            "edit.program",
            &context,
            &json!({"source_id":source["result"]["source_id"],"rules":[]}),
        )
        .await
        .unwrap();
    core.operate(
        "cut.publish",
        &context,
        &json!({"job_id":job["result"]["job_id"],"expected_cut":0,"idempotency_key":"first"}),
    )
    .await
    .unwrap();
    let bridge = DshBridge::new(
        store,
        Arc::new(CoreRuntime::new(core.clone())),
        DshBridgeConfig {
            model_alias: "sic-worker".into(),
            fixed_max_tokens: Some(4096),
            max_worker_calls_per_turn: 4,
            max_host_tasks_per_response: 4,
            max_harness_tools: 128,
            fixed_reasoning_effort: None,
        },
    )
    .await
    .unwrap();
    bridge
        .bind_existing_session("owner", "session", &chain)
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-sic-bridge-session", HeaderValue::from_static("session"));
    let body = json!({"model":"sic-worker","messages":[{"role":"system","content":"SYNTHETIC_NEW_PROTECTED"},
        {"role":"user","content":"continue"}],"stream":true,"tools":[],"tool_choice":"auto","max_tokens":4096});
    let error = bridge.handle("owner", &headers, &body).await.unwrap_err();
    assert_eq!(error.code, "protected_prefix_conflict");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(!error.message.contains("SYNTHETIC_"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let state: Value = core
        .operate("chain.get", &context, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        state["result"]["protected"][0]["content"],
        "SYNTHETIC_OLD_PROTECTED"
    );
    assert_eq!(state["result"]["cut_seq"], 1);
}

#[tokio::test]
async fn core_protected_prefix_conflicts_use_static_error_code_for_both_branches() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut context = OperationContext {
        actor: "owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    };
    let chain = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    context.chain_id = Some(chain);

    core.operate(
        "source.ingest",
        &context,
        &json!({"request":{"messages":[
            {"role":"system","content":"old protected"},
            {"role":"user","content":"initial"}
        ]}}),
    )
    .await
    .unwrap();
    let changed = core
        .operate(
            "source.ingest",
            &context,
            &json!({"request":{"messages":[
                {"role":"system","content":"different protected"},
                {"role":"user","content":"continue"}
            ]}}),
        )
        .await
        .unwrap_err();
    assert_eq!(changed.code, "protected_prefix_conflict");
    assert_eq!(changed.status, 409);

    let fresh = core
        .operate(
            "chain.create",
            &OperationContext {
                chain_id: None,
                ..context.clone()
            },
            &json!({}),
        )
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut fresh_context = context.clone();
    fresh_context.chain_id = Some(fresh);
    let source = core
        .operate(
            "source.ingest",
            &fresh_context,
            &json!({"request":{"messages":[{"role":"user","content":"initial"}]}}),
        )
        .await
        .unwrap()["result"]["source_id"]
        .clone();
    let job = core
        .operate(
            "edit.program",
            &fresh_context,
            &json!({"source_id":source,"rules":[]}),
        )
        .await
        .unwrap()["result"]["job_id"]
        .clone();
    core.operate(
        "cut.publish",
        &fresh_context,
        &json!({"job_id":job,"expected_cut":0,"idempotency_key":"prefix-test"}),
    )
    .await
    .unwrap();
    let added_late = core
        .operate(
            "source.ingest",
            &fresh_context,
            &json!({"request":{"messages":[
                {"role":"system","content":"late protected"},
                {"role":"user","content":"continue"}
            ]}}),
        )
        .await
        .unwrap_err();
    assert_eq!(added_late.code, "protected_prefix_conflict");
    assert_eq!(added_late.status, 409);
}
