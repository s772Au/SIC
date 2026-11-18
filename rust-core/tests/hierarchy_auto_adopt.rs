use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    scheduler::{SchedulerPolicy, schedule},
    storage::Store,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone)]
struct NativeCalls {
    requests: Arc<Mutex<Vec<Value>>>,
    core: Arc<Mutex<Option<(Core, OperationContext)>>>,
    hold_once: Arc<AtomicBool>,
    entered: Arc<Notify>,
    gate: Arc<Semaphore>,
}
fn activation(request: &Value) -> (usize, Value) {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .find_map(|(index, message)| {
            let parsed: Value = serde_json::from_str(message["content"].as_str()?).ok()?;
            parsed
                .get("sic_host_activation")
                .cloned()
                .map(|value| (index, value))
        })
        .expect("actual model request must carry trusted activation")
}
async fn save_requests(calls: &NativeCalls, name: &str) {
    if let Some(root) = std::env::var_os("SIC_TEST_EVIDENCE_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(name),
            serde_json::to_vec_pretty(&*calls.requests.lock().await).unwrap(),
        )
        .unwrap();
    }
}
async fn completion(State(state): State<NativeCalls>, Json(request): Json<Value>) -> Json<Value> {
    let (_, auth) = activation(&request);
    let instrumented = state.core.lock().await.clone();
    if let Some((core, context)) = instrumented {
        let current = op(&core, &context, "cut.get", json!({})).await;
        let cut = current["cut_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("genesis:{}", context.chain_id.as_deref().unwrap()));
        assert_eq!(
            auth["cb_boundary"]["cut_id"], cut,
            "actual dispatch must use the current common cut"
        );
        let selected = current["view"]["cbs"].as_array().unwrap();
        assert_eq!(
            auth["cb_boundary"]["cb_ids"],
            json!(
                selected
                    .iter()
                    .map(|cb| cb["cb_id"].clone())
                    .collect::<Vec<_>>()
            )
        );
        let (boundary, _) = activation(&request);
        let prefix = request["messages"].as_array().unwrap()[..boundary].to_vec();
        assert_eq!(prefix.len(), selected.len() + 1);
        for (message, cb) in prefix[1..].iter().zip(selected) {
            assert_eq!(message["content"], cb["body"]);
        }
    }
    state.requests.lock().await.push(request);
    if auth["mode"] == "editor" && state.hold_once.swap(false, Ordering::SeqCst) {
        state.entered.notify_one();
        state.gate.acquire().await.unwrap().forget();
    }
    let message = if auth["mode"] == "editor" {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"native-edit","type":"function",
            "function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[],\"pvi\":{}}"}}]})
    } else {
        json!({"role":"assistant","content":"worker continuation"})
    };
    Json(
        json!({"model":"fixture-model","choices":[{"finish_reason":if auth["mode"]=="editor" {"tool_calls"}else{"stop"},"message":message}]}),
    )
}
async fn fixture() -> (
    Core,
    OperationContext,
    NativeCalls,
    tokio::task::JoinHandle<()>,
) {
    let state = NativeCalls {
        requests: Arc::default(),
        core: Arc::new(Mutex::new(None)),
        hold_once: Arc::new(AtomicBool::new(false)),
        entered: Arc::new(Notify::new()),
        gate: Arc::new(Semaphore::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let profile = json!({"model":"openai/fixture-model","api_base":format!("http://{address}/v1"),"api_key_env":"FIXTURE_ONLY",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native","context_window":1_000_000,
        "max_input_tokens":1_000_000,"max_output_tokens":65536,"default_output_tokens":4096,"timeout_seconds":10});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile.clone(),"editor":profile}}),
        Arc::new(|_| Ok("local-fixture".into())),
    )
    .unwrap();
    let mut auth = OperationContext {
        actor: "recursive-owner".into(),
        allow_write: true,
        allow_model: true,
        ..Default::default()
    };
    let created = op(&core, &auth, "chain.create", json!({})).await;
    auth.chain_id = created["chain_id"].as_str().map(str::to_owned);
    *state.core.lock().await = Some((core.clone(), auth.clone()));
    (core, auth, state, server)
}
async fn op(core: &Core, auth: &OperationContext, name: &str, payload: Value) -> Value {
    core.operate(name, auth, &payload).await.unwrap()["result"].clone()
}
async fn ingest(core: &Core, auth: &OperationContext, text: &str) -> String {
    op(
        core,
        auth,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn program_job(core: &Core, auth: &OperationContext, source: &str, pvi: Value) -> String {
    op(
        core,
        auth,
        "edit.program",
        json!({"source_id":source,"rules":[],"pvi":pvi}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn publish(core: &Core, auth: &OperationContext, job: &str, key: &str) -> Value {
    let cut = op(core, auth, "chain.get", json!({})).await["cut_seq"].clone();
    op(
        core,
        auth,
        "cut.publish",
        json!({"job_id":job,"expected_cut":cut,"idempotency_key":key}),
    )
    .await
}
fn policy() -> SchedulerPolicy {
    SchedulerPolicy {
        max_sources_per_wb: 1,
        max_level: 3,
        level_batch_sizes: [(2, 2), (3, 2)].into(),
        max_actions: 256,
        max_model_calls: 64,
        ..Default::default()
    }
}

#[tokio::test]
async fn scheduler_automatically_adopts_three_levels_growth_tails_and_every_role_request() {
    let (core, auth, calls, server) = fixture().await;
    for number in 0..13 {
        ingest(&core, &auth, &format!("fact-{number}")).await;
        let report = schedule(&core, &auth, &policy()).await;
        assert!(report.failure.is_none(), "{:?}", report.failure);
        assert!(
            !report
                .operations
                .iter()
                .any(|name| name == "hierarchy.select"),
            "adoption must be a publication invariant, not an extra caller operation"
        );
        let state = op(&core, &auth, "chain.get", json!({})).await;
        let expected_level = if number >= 3 {
            3
        } else if number >= 1 {
            2
        } else {
            1
        };
        assert_eq!(state["role_views"]["worker"]["level"], expected_level);
        assert_eq!(state["role_views"]["worker"], state["role_views"]["editor"]);
        let view = op(&core, &auth, "cut.get", json!({})).await;
        let text = view["view"]["cbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cb| cb["body"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            (0..=number)
                .map(|n| format!("fact-{n}"))
                .collect::<Vec<_>>()
        );
    }
    let timeline = op(
        &core,
        &auth,
        "hierarchy.timeline",
        json!({"context_id":"main","level":3,"kind":"cb"}),
    )
    .await;
    assert_eq!(timeline["total"], 3);
    let low = op(
        &core,
        &auth,
        "hierarchy.timeline",
        json!({"context_id":"main","level":1,"kind":"cb"}),
    )
    .await;
    let mut expected: Vec<Value> = timeline["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].clone())
        .collect();
    expected.push(low["rows"][12]["id"].clone());
    let cut = op(&core, &auth, "cut.get", json!({})).await;
    assert_eq!(
        cut["view"]["cbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cb| cb["cb_id"].clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        cut["cut"]["role_views"]["worker"],
        cut["cut"]["role_views"]["editor"]
    );
    assert_eq!(
        cut["view"]["cb_origin_bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|binding| binding["missing_source_ids"].as_array().unwrap().len())
            .sum::<usize>(),
        13,
        "legacy sources are explicitly unmapped; higher CBs report their leaf sources rather than inventing SDK IDs"
    );
    let old = op(
        &core,
        &auth,
        "memory.replay",
        json!({"cb_id":low["rows"][0]["id"]}),
    )
    .await;
    assert_eq!(old["body"], "fact-0");
    assert_eq!(old["body_matches_published_cb"], true);

    save_requests(&calls, "hierarchy-scheduler-continuous-requests.json").await;
    calls.requests.lock().await.clear();
    let worker = core.operate("worker.run", &auth, &json!({})).await.unwrap();
    let extra=op(&core,&auth,"source.ingest",json!({"request":{"messages":[{"role":"user","content":"new-a"},{"role":"user","content":"new-b"}]}})).await;
    let source = extra["source_id"].as_str().unwrap();
    let grouped = op(
        &core,
        &auth,
        "wb.form",
        json!({"source_ids":[source],"include_earlier_pending":true}),
    )
    .await;
    let prepared=op(&core,&auth,"edit.prepare",json!({"source_id":grouped["source_id"],"assignments":[
        {"editor":"e1","unit_ids":["u1"]},{"editor":"e2","unit_ids":[format!("{source}:u1"),format!("{source}:u2")]}
    ]})).await;
    let first = core
        .operate(
            "edit.run",
            &auth,
            &json!({"job_id":prepared["job_id"],"editor":"e1"}),
        )
        .await
        .unwrap();
    let second = core
        .operate(
            "edit.run",
            &auth,
            &json!({"job_id":prepared["job_id"],"editor":"e2"}),
        )
        .await
        .unwrap();
    assert_eq!(
        worker["evidence"]["common_prefix_digest"],
        first["evidence"]["common_prefix_digest"]
    );
    assert_eq!(
        first["evidence"]["common_prefix_digest"],
        second["evidence"]["common_prefix_digest"]
    );
    let requests = calls.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let mut common = None;
    for request in requests.iter() {
        let (boundary, activation) = activation(request);
        assert_eq!(activation["cb_boundary"]["cb_ids"], json!(expected));
        assert_eq!(activation["cb_boundary"]["cut_id"], cut["cut_id"]);
        let prefix = json!({"model":request["model"],"tools":request["tools"],"messages":request["messages"].as_array().unwrap()[..boundary]});
        if let Some(prior) = &common {
            assert_eq!(&prefix, prior);
        } else {
            common = Some(prefix);
        }
    }
    drop(requests);
    save_requests(&calls, "hierarchy-worker-all-editors-requests.json").await;
    server.abort();
}

#[tokio::test]
async fn publication_adopts_atomically_preserves_inflight_snapshot_and_carries_instant_views() {
    let (core, auth, calls, server) = fixture().await;
    let mut base = Vec::new();
    for n in 0..4 {
        let source = ingest(&core, &auth, &format!("base-{n}")).await;
        let job = program_job(
            &core,
            &auth,
            &source,
            json!({"I":{"status":"latest-base-state"}}),
        )
        .await;
        base.push(publish(&core, &auth, &job, &format!("base-{n}")).await["cb_id"].clone());
    }
    let higher = op(
        &core,
        &auth,
        "hierarchy.form",
        json!({"level":2,"cb_ids":base[..2]}),
    )
    .await;
    let high_job = program_job(
        &core,
        &auth,
        higher["source_id"].as_str().unwrap(),
        json!({}),
    )
    .await;
    let pending = ingest(&core, &auth, "new-editor-material").await;
    let low = op(&core, &auth, "edit.prepare", json!({"source_id":pending})).await;
    calls.hold_once.store(true, Ordering::SeqCst);
    let running = {
        let core = core.clone();
        let auth = auth.clone();
        let low = low.clone();
        tokio::spawn(async move {
            core.operate("edit.run", &auth, &json!({"job_id":low["job_id"]}))
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), calls.entered.notified())
        .await
        .unwrap();
    let before = calls.requests.lock().await.last().unwrap().clone();
    let result = publish(&core, &auth, &high_job, "automatic-high").await;
    assert_eq!(result["shared_representation"]["automatic_promotion"], true);
    let cut = op(&core, &auth, "cut.get", json!({})).await;
    assert_eq!(cut["cut"]["role_views"]["worker"]["level"], 2);
    assert_eq!(cut["view"]["instant"]["status"], "latest-base-state");
    assert_eq!(
        cut["view"]["cbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cb| cb["cb_id"].clone())
            .collect::<Vec<_>>(),
        vec![result["cb_id"].clone(), base[2].clone(), base[3].clone()]
    );
    assert_eq!(activation(&before).1["cb_boundary"]["cb_ids"], json!(base));
    assert_eq!(
        calls.requests.lock().await.last().unwrap(),
        &before,
        "in-flight request remains its original frozen representation"
    );
    calls.gate.add_permits(1);
    assert!(
        running.await.unwrap().is_err(),
        "old-cut response cannot overwrite a newer publication"
    );
    let next = op(&core, &auth, "edit.prepare", json!({"source_id":pending})).await;
    core.operate("edit.run", &auth, &json!({"job_id":next["job_id"]}))
        .await
        .unwrap();
    let request = calls.requests.lock().await.last().unwrap().clone();
    assert_eq!(
        activation(&request).1["cb_boundary"]["cut_id"],
        result["cut_id"]
    );
    assert_eq!(
        activation(&request).1["cb_boundary"]["cb_ids"],
        result["shared_representation"]["selected_cb_ids"]
    );
    save_requests(&calls, "hierarchy-inflight-before-after-requests.json").await;
    server.abort();
}

#[tokio::test]
async fn native_scheduler_recovers_an_existing_unadopted_high_view_without_another_editor_call() {
    let (core, auth, calls, server) = fixture().await;
    for n in 0..4 {
        ingest(&core, &auth, &format!("legacy-{n}")).await;
    }
    assert!(schedule(&core, &auth, &policy()).await.failure.is_none());
    let high = op(&core, &auth, "cut.get", json!({})).await;
    let already=op(&core,&auth,"hierarchy.select",json!({"level":3,"roles":["editor"]})).await;
    assert_eq!(already["sequence"],high["sequence"],"old helper selections are idempotent once publication already adopted the same common view");
    assert_eq!(already["changed"],false);
    let low = op(
        &core,
        &auth,
        "hierarchy.select",
        json!({"level":1,"roles":["worker"]}),
    )
    .await;
    assert_eq!(
        low["role_views"]["worker"], low["role_views"]["editor"],
        "a public role selection cannot split the common prefix"
    );
    assert_eq!(low["role_views"]["worker"]["level"], 1);
    let before = calls.requests.lock().await.len();
    let repaired = schedule(&core, &auth, &policy()).await;
    assert!(repaired.failure.is_none(), "{:?}", repaired.failure);
    assert!(repaired.published.is_empty());
    assert!(
        repaired
            .operations
            .iter()
            .any(|name| name == "hierarchy.select")
    );
    assert_eq!(
        calls.requests.lock().await.len(),
        before,
        "recovering adoption does not rerun editing"
    );
    let adopted = op(&core, &auth, "cut.get", json!({})).await;
    assert_eq!(adopted["cut"]["role_views"]["worker"]["level"], 3);
    assert_eq!(adopted["view"]["cbs"], high["view"]["cbs"]);
    server.abort();
}

#[tokio::test]
async fn high_cb_origin_bindings_follow_only_covered_sdk_units_and_keep_the_native_tail() {
    let (core, auth, _calls, server) = fixture().await;
    let mut base = Vec::new();
    for n in 0..4 {
        let cut = op(&core, &auth, "cut.get", json!({})).await;
        let previous = cut["view"]["cbs"].as_array().unwrap();
        let mut messages: Vec<Value> = previous
            .iter()
            .map(|cb| json!({"role":"user","content":cb["body"]}))
            .collect();
        let index = messages.len();
        messages.push(json!({"role":"user","content":format!("native-{n}")}));
        let source=op(&core,&auth,"native.begin",json!({"format":"openai-chat","request":{"messages":messages},
            "editable_anchors":[["messages",index,"content"]],
            "protected_cb_bindings":previous.iter().enumerate().map(|(i,cb)|json!({"cb_id":cb["cb_id"],"native_anchors":[["messages",i,"content"]]})).collect::<Vec<_>>(),
            "host_message_origins":[{"message_id":format!("sdk-{n}"),"block_index":n,"native_anchor":["messages",index,"content"]}]})).await;
        op(&core,&auth,"native.commit",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,
            "edits":[],"delivery":{"ops":[],"pvi":{}}})).await;
        let done=op(&core,&auth,"native.publish",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,
            "expected_cut":cut["sequence"],"idempotency_key":format!("native-base-{n}")})).await;
        base.push(done["publication"]["cb_id"].clone());
    }
    let mut twos = Vec::new();
    for (n, pair) in base.chunks(2).enumerate() {
        let formed = op(
            &core,
            &auth,
            "hierarchy.form",
            json!({"level":2,"cb_ids":pair}),
        )
        .await;
        let job = program_job(
            &core,
            &auth,
            formed["source_id"].as_str().unwrap(),
            json!({}),
        )
        .await;
        twos.push(publish(&core, &auth, &job, &format!("native-two-{n}")).await["cb_id"].clone());
    }
    let formed = op(
        &core,
        &auth,
        "hierarchy.form",
        json!({"level":3,"cb_ids":twos}),
    )
    .await;
    let job = program_job(
        &core,
        &auth,
        formed["source_id"].as_str().unwrap(),
        json!({}),
    )
    .await;
    let high = publish(&core, &auth, &job, "native-three").await;
    let selected = op(&core, &auth, "cut.get", json!({})).await;
    let binding = &selected["view"]["cb_origin_bindings"][0];
    assert_eq!(binding["cb_id"], high["cb_id"]);
    assert_eq!(binding["missing_source_ids"], json!([]));
    assert_eq!(
        binding["origins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|origin| origin["message_id"].clone())
            .collect::<Vec<_>>(),
        (0..4)
            .map(|n| json!(format!("sdk-{n}")))
            .collect::<Vec<_>>()
    );
    let source=op(&core,&auth,"native.begin",json!({"format":"openai-chat","request":{"messages":[
        {"role":"user","content":selected["view"]["cbs"][0]["body"]},{"role":"user","content":"native-tail"}]},
        "editable_anchors":[["messages",1,"content"]],"protected_cb_bindings":[{"cb_id":high["cb_id"],"native_anchors":[["messages",0,"content"]]}],
        "host_message_origins":[{"message_id":"sdk-tail","block_index":0,"native_anchor":["messages",1,"content"]}]})).await;
    op(&core,&auth,"native.commit",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,"edits":[]})).await;
    op(&core,&auth,"native.publish",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,"expected_cut":selected["sequence"],"idempotency_key":"native-tail"})).await;
    let tail = op(&core, &auth, "cut.get", json!({})).await;
    assert_eq!(
        tail["view"]["cb_origin_bindings"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        tail["view"]["cb_origin_bindings"][1]["origins"][0]["message_id"],
        "sdk-tail"
    );
    assert_eq!(tail["view"]["cbs"][0]["cb_id"], high["cb_id"]);
    server.abort();
}
