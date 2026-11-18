use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;

async fn completion(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({
        "model": "deepseek-v4.1-flash-eco",
        "usage": {
            "prompt_tokens": 4000,
            "completion_tokens": 200,
            "prompt_tokens_details": {"cached_tokens": 3000}
        },
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "answer"}}]
    }))
}

async fn fixture() -> (Core, OperationContext, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(()),
        )
        .await;
    });
    let profile = json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "api_base": format!("http://{address}/v1"),
        "api_key_env": "SIC_TEST_MOCK_KEY",
        "tokenizer_model": "host-o200k-surrogate",
        "tokenizer_status": "surrogate-not-provider-native",
        "context_window": 1_000_000,
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 65536,
        "default_output_tokens": 4096,
        "timeout_seconds": 10
    });
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("local-fixture-key".into())),
    )
    .unwrap();
    let mut context = OperationContext {
        actor: "owner".into(),
        chain_id: None,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    };
    let created = core
        .operate("chain.create", &context, &json!({}))
        .await
        .unwrap();
    context.chain_id = Some(created["result"]["chain_id"].as_str().unwrap().into());
    (core, context, server)
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}

#[tokio::test]
async fn accounting_exposes_session_statistics_from_provider_usage() {
    let (core, ctx, server) = fixture().await;

    // An empty chain has no attempts, so every statistic that needs one stays
    // null instead of collapsing to a confident zero.
    let empty = call(&core, &ctx, "accounting.get", json!({})).await;
    let stats = &empty["stats"];
    assert_eq!(stats["class"], "host_measured");
    assert_eq!(stats["steps"], 0);
    assert_eq!(stats["prompt_tokens"]["known"], 0);
    assert_eq!(stats["prompt_tokens"]["complete"], false);
    assert_eq!(stats["cache_hit_rate"], Value::Null);
    assert_eq!(stats["tokens_per_second"], Value::Null);
    assert_eq!(stats["context_tokens"], Value::Null);
    assert_eq!(stats["context_window"], 1_000_000);
    assert_eq!(stats["context_percent"], Value::Null);
    assert_eq!(stats["turns"], Value::Null);
    let unknowns = stats["unknowns"].as_array().unwrap();
    assert!(unknowns.contains(&json!("turn_count_owned_by_host_bridge")));
    assert!(unknowns.contains(&json!("prompt_tokens_incomplete")));

    // One recorded Worker call populates the counters from the provider's own
    // usage report; cache reads are a subset of prompt tokens, not a new bucket.
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"question"}]}}),
    )
    .await;
    let source_id = source["source_id"].as_str().unwrap().to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source_id]})).await;
    core.operate("worker.run", &ctx, &json!({})).await.unwrap();

    let after = call(&core, &ctx, "accounting.get", json!({})).await;
    let stats = &after["stats"];
    assert_eq!(stats["steps"], 1);
    assert_eq!(stats["steps_by_role"]["worker"], 1);
    assert_eq!(stats["prompt_tokens"]["known"], 4000);
    assert_eq!(stats["prompt_tokens"]["complete"], true);
    assert_eq!(stats["output_tokens"]["known"], 200);
    assert_eq!(stats["cache_read_tokens"]["known"], 3000);
    assert_eq!(stats["total_tokens"]["known"], 4200);
    let hit = stats["cache_hit_rate"].as_f64().unwrap();
    assert!((hit - 0.75).abs() < 1e-9, "{hit}");
    assert_eq!(stats["context_tokens"], 4000);
    let percent = stats["context_percent"].as_f64().unwrap();
    assert!((percent - 0.004).abs() < 1e-9, "{percent}");
    // A whole-call rate needs a measured interval, never an inferred duration.
    assert_eq!(stats["tokens_per_second_class"], "whole_call_output_over_model_seconds_not_decode");
    assert_eq!(stats["basis"]["cache"], "provider_reported_cached_prompt_tokens_subset_of_prompt");
    assert!(
        stats["non_claims"]
            .as_array()
            .unwrap()
            .contains(&json!("no_price_or_bill"))
    );
    server.abort();
}

#[tokio::test]
async fn turns_come_from_bridge_claims_scoped_to_this_chain() {
    let (core, ctx, server) = fixture().await;
    let chain = ctx.chain_id.clone().unwrap();

    // A chain no bridge ever claimed keeps `turns` null, not a confident zero.
    let before = call(&core, &ctx, "accounting.get", json!({})).await;
    assert_eq!(before["stats"]["turns"], Value::Null);

    // The bridge owns the user-turn boundary. Seed its own two tables with the
    // same shape the bridge creates: two delivered claims under this chain and
    // one under a different chain, so the count follows the chain, not the process.
    let mut tx = core.store.begin().await.unwrap();
    sqlx::query("CREATE TABLE IF NOT EXISTS dsh_bridge_sessions(actor TEXT NOT NULL,session_key TEXT NOT NULL,model_alias TEXT NOT NULL,chain_id TEXT,history TEXT NOT NULL,pending_sources TEXT NOT NULL,active_fingerprint TEXT,tools_fingerprint TEXT NOT NULL DEFAULT '',worker_calls INTEGER NOT NULL DEFAULT 0,host_task_count INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(actor,session_key))")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE IF NOT EXISTS dsh_bridge_turns(actor TEXT NOT NULL,session_key TEXT NOT NULL,fingerprint TEXT NOT NULL,state TEXT NOT NULL,phase TEXT NOT NULL,progress TEXT NOT NULL,response_sse TEXT,error_code TEXT,created INTEGER NOT NULL,PRIMARY KEY(actor,session_key,fingerprint))")
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows: [(&str, &str, &str); 3] = [
        ("host", "s1", chain.as_str()),
        ("host", "s1", chain.as_str()),
        ("host", "s2", "chain_other"),
    ];
    for (index, (actor, session, chain_id)) in rows.iter().enumerate() {
        sqlx::query("INSERT OR REPLACE INTO dsh_bridge_sessions(actor,session_key,model_alias,chain_id,history,pending_sources,active_fingerprint,tools_fingerprint,worker_calls,host_task_count) VALUES(?,?,?,?,?,?,?,?,0,0)")
            .bind(actor)
            .bind(session)
            .bind("model")
            .bind(chain_id)
            .bind("[]")
            .bind("[]")
            .bind(Option::<String>::None)
            .bind("")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT OR REPLACE INTO dsh_bridge_turns(actor,session_key,fingerprint,state,phase,progress,created) VALUES(?,?,?,?,?,?,?)")
            .bind(actor)
            .bind(session)
            .bind(format!("fingerprint-{index}"))
            .bind("delivered")
            .bind("done")
            .bind("{}")
            .bind(1i64)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    let after = call(&core, &ctx, "accounting.get", json!({})).await;
    let stats = &after["stats"];
    assert_eq!(stats["turns"], 2, "only the claims under this chain count");
    let unknowns = stats["unknowns"].as_array().unwrap();
    assert!(!unknowns.contains(&json!("turn_count_owned_by_host_bridge")));
    server.abort();
}
