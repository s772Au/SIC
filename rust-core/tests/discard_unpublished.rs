use axum::{Json, Router, http::StatusCode, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;

async fn provider_500(Json(_body): Json<Value>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error":{"code":"provider_server_error","message":"synthetic 500"}})),
    )
}

fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "alice".into(),
        chain_id: chain,
        allow_write: true,
        allow_model: true,
        allow_revoke: false,
    }
}

async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}

#[tokio::test]
async fn discard_unpublished_clears_only_the_failed_first_turn() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(provider_500)),
        )
        .await;
    });
    let store = Store::memory().await.unwrap();
    let profile = json!({
        "model":"openai/mock",
        "api_base":format!("http://{address}/v1"),
        "api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate",
        "context_window":32000,
        "max_input_tokens":32000,
        "max_output_tokens":2048,
        "default_output_tokens":128,
        "timeout_seconds":10,
        "retry_editor_http_5xx":false
    });
    let core = Core::with_model_config_and_resolver(
        store,
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"editable first-turn material"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        core.operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
            .await
            .is_err()
    );
    let before = call(&core, &ctx, "chain.get", json!({})).await;
    let level = &before["contexts"]["main"]["levels"]["1"];
    assert!(!level["pending"].as_array().unwrap().is_empty());
    assert!(!level["wbs"].as_array().unwrap().is_empty());
    assert!(!before["jobs"].as_object().unwrap().is_empty());

    assert!(
        core.operate(
            "runtime.discard_unpublished",
            &ctx,
            &json!({"confirm":false,"reason":"must not discard without confirmation"})
        )
        .await
        .is_err(),
        "confirm=false must be refused"
    );
    let discarded = call(
        &core,
        &ctx,
        "runtime.discard_unpublished",
        json!({"confirm":true,"reason":"operator recovered a failed first turn"}),
    )
    .await;
    assert_eq!(discarded["discarded"], true);
    assert!(discarded["cleared_sources"].as_u64().unwrap() >= 1);
    assert!(discarded["discarded_wbs"].as_array().unwrap().len() >= 1);

    let after = call(&core, &ctx, "chain.get", json!({})).await;
    let level = &after["contexts"]["main"]["levels"]["1"];
    assert!(level["pending"].as_array().unwrap().is_empty());
    assert!(level["wbs"].as_array().unwrap().is_empty());
    assert!(after["jobs"].as_object().unwrap().is_empty());
    assert_eq!(after["run_state"], "running");
    server.abort();
}

#[tokio::test]
async fn discard_unpublished_preserves_an_already_published_cut() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(provider_500)),
        )
        .await;
    });
    let store = Store::memory().await.unwrap();
    let profile = json!({
        "model":"openai/mock",
        "api_base":format!("http://{address}/v1"),
        "api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-surrogate",
        "context_window":32000,
        "max_input_tokens":32000,
        "max_output_tokens":2048,
        "default_output_tokens":128,
        "timeout_seconds":10,
        "retry_editor_http_5xx":false
    });
    let core = Core::with_model_config_and_resolver(
        store,
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("fixture-key".into())),
    )
    .unwrap();
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"published first cut"}]}}),
    )
    .await;
    let scheduled = call(
        &core,
        &ctx,
        "runtime.schedule",
        json!({"context_id":"main","min_sources_per_wb":1,"max_sources_per_wb":4,
               "max_units_per_wb":512,"level_batch_sizes":{},"max_level":1,
               "flush":true,"max_actions":16,"max_model_calls":0,
               "editor_mode":"program_fixture","program_rules":["deduplicate_exact"]}),
    )
    .await;
    assert_eq!(scheduled["published"].as_array().unwrap().len(), 1);
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"unpublished failed second turn"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        core.operate(
            "runtime.discard_unpublished",
            &ctx,
            &json!({"confirm":true,"reason":"must not discard an open job without opt-in"})
        )
        .await
        .is_err(),
        "an open current-cut job must block discard by default"
    );
    assert!(
        core.operate("edit.run", &ctx, &json!({"job_id":job,"editor":"e1"}))
            .await
            .is_err()
    );
    let before = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(before["cut_seq"], 1);
    assert_eq!(
        before["contexts"]["main"]["levels"]["1"]["cbs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        !before["contexts"]["main"]["levels"]["1"]["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    call(
        &core,
        &ctx,
        "runtime.discard_unpublished",
        json!({"confirm":true,"reason":"operator recovered a failed later turn"}),
    )
    .await;
    let after = call(&core, &ctx, "chain.get", json!({})).await;
    let level = &after["contexts"]["main"]["levels"]["1"];
    assert_eq!(after["cut_seq"], 1);
    assert_eq!(level["cbs"].as_array().unwrap().len(), 1);
    assert!(level["pending"].as_array().unwrap().is_empty());
    let wbs = level["wbs"].as_array().unwrap();
    assert_eq!(wbs.len(), 1, "the published cut's paired WB is retained");
    assert!(
        level["pairs"].get(wbs[0].as_str().unwrap()).is_some(),
        "retained WB must remain paired"
    );
    let jobs = after["jobs"].as_object().unwrap();
    assert!(
        !jobs.contains_key(&job),
        "the failed current-cut job is removed"
    );
    assert_eq!(jobs.len(), 1, "the published job is retained");
    server.abort();
}
#[tokio::test]
async fn discard_open_current_job_requires_explicit_opt_in() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"open job material"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected = core
        .operate(
            "runtime.discard_unpublished",
            &ctx,
            &json!({"confirm":true,"reason":"default must reject open"}),
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "invalid");
    let discarded = call(
        &core,
        &ctx,
        "runtime.discard_unpublished",
        json!({"confirm":true,"allow_open":true,"reason":"explicit open recovery"}),
    )
    .await;
    assert_eq!(discarded["discarded"], true);
    assert!(
        discarded["discarded_jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &job)
    );
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 0);
    assert!(
        state["contexts"]["main"]["levels"]["1"]["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store.close().await;
}
#[tokio::test]
async fn discard_inflight_current_job_requires_allow_open() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain.clone()));
    let source = call(
        &core,
        &ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"inflight job material"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(&core, &ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        &core,
        &ctx,
        "edit.prepare",
        json!({"source_id":source,"assignments":[{"editor":"e1","unit_ids":["u1"]}]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tx = store.begin().await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM chains WHERE id=?")
        .bind(&chain)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let mut value: Value = serde_json::from_str(&state).unwrap();
    value["jobs"][&job]["status"] = json!("inflight");
    sqlx::query("UPDATE chains SET state=? WHERE id=?")
        .bind(serde_json::to_string(&value).unwrap())
        .bind(&chain)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        core.operate(
            "runtime.discard_unpublished",
            &ctx,
            &json!({"confirm":true,"reason":"must reject inflight by default"})
        )
        .await
        .is_err(),
        "inflight current-cut job must require opt-in"
    );
    let discarded = call(
        &core,
        &ctx,
        "runtime.discard_unpublished",
        json!({"confirm":true,"allow_open":true,"reason":"explicit inflight recovery"}),
    )
    .await;
    assert_eq!(discarded["discarded"], true);
    assert!(
        discarded["discarded_jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &job)
    );
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 0);
    assert!(
        state["contexts"]["main"]["levels"]["1"]["pending"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store.close().await;
}
