use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn context(
    actor: &str,
    chain: Option<&str>,
    allow_model: bool,
    allow_revoke: bool,
) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model,
        allow_revoke,
    }
}

async fn call(core: &Core, ctx: &OperationContext, operation: &str, payload: Value) -> Value {
    core.operate(operation, ctx, &payload)
        .await
        .unwrap_or_else(|e| panic!("{operation} failed: {e:?}"))["result"]
        .clone()
}

async fn make_chain(core: &Core, actor: &str) -> String {
    call(
        core,
        &context(actor, None, false, false),
        "chain.create",
        json!({}),
    )
    .await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish(core: &Core, ctx: &OperationContext, text: &str, key: &str) -> Value {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":[]}),
    )
    .await;
    let state = call(core, ctx, "chain.get", json!({})).await;
    call(
        core,
        ctx,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await
}

#[tokio::test]
async fn socket_read_pages_the_frozen_checkpoint_and_rechecks_acl_and_revocation() {
    let core = Core::new(Store::memory().await.unwrap());
    let alice_id = make_chain(&core, "alice").await;
    let bob_id = make_chain(&core, "bob").await;
    let carol_id = make_chain(&core, "carol").await;
    let alice = context("alice", Some(&alice_id), false, true);
    let bob = context("bob", Some(&bob_id), false, false);
    let carol = context("carol", Some(&carol_id), false, false);

    let first = publish(&core, &alice, "alpha checkpoint body", "cp-one").await;
    let cp1 = call(
        &core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":first["cut_id"]}),
    )
    .await;
    let socket1 = call(
        &core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":cp1["checkpoint_id"],"acl":{
            "message":[],"signal":[],"spawn":[],"read":[bob_id]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let second = publish(&core, &alice, "beta checkpoint body", "cp-two").await;
    let cp2 = call(
        &core,
        &alice,
        "chain.checkpoint",
        json!({"cut_id":second["cut_id"]}),
    )
    .await;
    let socket2 = call(
        &core,
        &alice,
        "socket.create",
        json!({"checkpoint_id":cp2["checkpoint_id"],"acl":{
            "message":[],"signal":[],"spawn":[],"read":[carol_id]
        }}),
    )
    .await["socket_id"]
        .as_str()
        .unwrap()
        .to_owned();
    publish(
        &core,
        &alice,
        "later body outside both checkpoints",
        "cp-three",
    )
    .await;

    let page1 = call(
        &core,
        &bob,
        "socket.read",
        json!({"socket_id":socket1,"offset":0,"limit":8}),
    )
    .await;
    assert_eq!(page1["cb_ids"].as_array().unwrap().len(), 1);
    assert_eq!(page1["total"], "alpha checkpoint body".chars().count());
    assert_eq!(page1["value"], "alpha ch");
    for bad_page in [
        json!({"offset":"0"}),
        json!({"offset":-1}),
        json!({"limit":"8"}),
        json!({"limit":null}),
    ] {
        let mut payload = bad_page;
        payload["socket_id"] = json!(socket1);
        assert!(core.operate("socket.read", &bob, &payload).await.is_err());
    }
    let page2 = call(
        &core,
        &bob,
        "socket.read",
        json!({"socket_id":socket1,"offset":page1["next_offset"],"limit":100}),
    )
    .await;
    assert_eq!(
        format!(
            "{}{}",
            page1["value"].as_str().unwrap(),
            page2["value"].as_str().unwrap()
        ),
        "alpha checkpoint body"
    );

    assert!(
        core.operate(
            "socket.read",
            &bob,
            &json!({"socket_id":socket2,"offset":0,"limit":100})
        )
        .await
        .is_err()
    );
    assert!(
        core.operate(
            "socket.read",
            &carol,
            &json!({"socket_id":socket1,"offset":0,"limit":100})
        )
        .await
        .is_err()
    );
    let cp2_read = call(
        &core,
        &carol,
        "socket.read",
        json!({"socket_id":socket2,"offset":0,"limit":100}),
    )
    .await;
    assert_eq!(cp2_read["cb_ids"].as_array().unwrap().len(), 2);
    assert_eq!(
        cp2_read["value"],
        "alpha checkpoint body\nbeta checkpoint body"
    );
    assert!(!cp2_read["value"].as_str().unwrap().contains("later body"));

    // ACL read is not enough after a dependency is revoked: the frozen snapshot
    // remains addressable, but Core rechecks source dependencies at read time.
    let revoked_source = second["source_ids"][0].as_str().unwrap();
    call(
        &core,
        &alice,
        "security.revoke",
        json!({"source_id":revoked_source,"reason":"fixture revocation"}),
    )
    .await;
    assert_eq!(
        core.operate(
            "socket.read",
            &carol,
            &json!({"socket_id":socket2,"offset":0,"limit":100})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
    assert!(
        core.operate(
            "socket.read",
            &bob,
            &json!({"socket_id":socket1,"offset":0,"limit":100})
        )
        .await
        .is_ok()
    );
}

#[derive(Clone)]
struct ModelFixture {
    response: Arc<Mutex<Value>>,
}

async fn completion(
    State(fixture): State<ModelFixture>,
    Json(_request): Json<Value>,
) -> Json<Value> {
    Json(fixture.response.lock().await.clone())
}

fn function_call(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}

fn call_response(id: &str, dynamic_name: &str, value: &str) -> Value {
    json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
        "tool_calls":[function_call(id,"sic_tool_call",json!({"name":dynamic_name,"arguments":{"value":value}}))]}}]})
}

fn grant(name: &str) -> Value {
    json!({"principal":"alice","tool_name":name})
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"host_fixed":false,"descriptor":{"name":name,"description":description,"schema":schema}})
}

fn schema(version: &str) -> Value {
    json!({"type":"object","properties":{"value":{"type":"string","description":version}},"required":["value"]})
}

fn sync_payload(
    tools: Vec<Value>,
    discover: Vec<Value>,
    expand: Vec<Value>,
    execute: Vec<Value>,
) -> Value {
    json!({"acl":{"discover":discover,"expand":expand,"execute":execute},"tools":tools})
}

fn acl_payload(discover: Vec<Value>, expand: Vec<Value>, execute: Vec<Value>) -> Value {
    json!({"acl":{"discover":discover,"expand":expand,"execute":execute}})
}

#[tokio::test]
async fn dynamic_catalog_sync_is_atomic_and_preserves_only_the_original_inflight_lease() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let initial = call_response("call-alpha-1", "alpha", "one");
    let response = Arc::new(Mutex::new(initial));
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(ModelFixture {
                    response: response.clone(),
                }),
        )
        .into_future(),
    );
    let profile = json!({"model":"openai/deepseek-v4.1-flash-eco","api_base":format!("http://{address}/v1"),"api_key_env":"LOCAL_FIXTURE_ONLY",
        "tokenizer_model":"host-o200k-surrogate","tokenizer_status":"surrogate-not-provider-native","context_window":100000,
        "max_input_tokens":100000,"max_output_tokens":4096,"default_output_tokens":512,"timeout_seconds":5});
    let core = Core::with_model_config_and_resolver(
        Store::memory().await.unwrap(),
        json!({"roles":{"worker":profile,"editor":profile}}),
        Arc::new(|_| Ok("fixture-only".into())),
    )
    .unwrap();
    let mut alice = context("alice", None, true, false);
    let chain_id = call(&core, &alice, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    alice.chain_id = Some(chain_id);
    let old_grant = grant("alpha");
    call(
        &core,
        &alice,
        "tools.configure",
        sync_payload(
            vec![tool("alpha", "alpha schema v1", schema("v1"))],
            vec![old_grant.clone()],
            vec![old_grant.clone()],
            vec![old_grant.clone()],
        ),
    )
    .await;
    call(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"run alpha"}]}}),
    )
    .await;
    let first = call(&core, &alice, "worker.run", json!({})).await;
    let task_id = first["host_tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(first["host_tasks"][0]["descriptor"]["schema"], schema("v1"));
    assert_eq!(
        first["host_tasks"][0]["descriptor"]["description"],
        "alpha schema v1"
    );

    let beta_grant = grant("beta");
    // ACL grants for a name absent from the proposed catalog must reject the
    // entire sync. No beta descriptor or ACL change may leak into persisted state.
    assert!(
        core.operate(
            "tools.sync",
            &alice,
            &sync_payload(
                vec![tool("beta", "beta proposed", schema("b1"))],
                vec![old_grant.clone(), beta_grant.clone()],
                vec![old_grant.clone(), beta_grant.clone()],
                vec![old_grant.clone(), beta_grant.clone()]
            )
        )
        .await
        .is_err()
    );
    let after_bad_acl = call(&core, &alice, "tools.status", json!({})).await;
    assert_eq!(after_bad_acl["catalog"].as_array().unwrap().len(), 1);
    assert_eq!(after_bad_acl["catalog"][0]["name"], "alpha");
    assert_eq!(after_bad_acl["active_leases"].as_array().unwrap().len(), 1);

    // A schema update for a tool with an active execution lease also rolls back
    // atomically, including the newly proposed beta tool.
    let rejected = core
        .operate(
            "tools.sync",
            &alice,
            &sync_payload(
                vec![
                    tool("alpha", "alpha schema v2", schema("v2")),
                    tool("beta", "beta newly proposed", schema("b1")),
                ],
                vec![old_grant.clone(), beta_grant.clone()],
                vec![old_grant.clone(), beta_grant.clone()],
                vec![old_grant.clone(), beta_grant.clone()],
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "conflict");
    let after_failure = call(&core, &alice, "tools.status", json!({})).await;
    assert_eq!(after_failure["catalog"].as_array().unwrap().len(), 1);
    assert_eq!(after_failure["catalog"][0]["name"], "alpha");
    assert_eq!(
        after_failure["catalog"][0]["description"],
        "alpha schema v1"
    );
    assert_eq!(after_failure["active_leases"].as_array().unwrap().len(), 1);

    call(
        &core,
        &alice,
        "tool.claim",
        json!({"task_id":task_id,"claim_id":"fixture-claim-alpha-1"}),
    )
    .await;
    let completed=call(&core,&alice,"tool.complete",json!({"task_id":task_id,"claim_id":"fixture-claim-alpha-1","succeeded":true,"result":{"fixture":"completed under original lease"}})).await;
    assert_eq!(completed["status"], "completed");
    let no_execute = sync_payload(
        vec![tool("alpha", "alpha schema v2", schema("v2"))],
        vec![old_grant.clone()],
        vec![old_grant.clone()],
        vec![],
    );
    call(&core, &alice, "tools.sync", no_execute).await;
    let updated = call(&core, &alice, "tools.status", json!({})).await;
    assert_eq!(updated["catalog"][0]["description"], "alpha schema v2");
    assert_eq!(
        updated["catalog"][0]["expanded"], false,
        "a schema change must invalidate expansion"
    );

    *response.lock().await = call_response("call-alpha-denied", "alpha", "two");
    let denied = call(&core, &alice, "worker.run", json!({})).await;
    assert!(denied["host_tasks"].as_array().unwrap().is_empty());
    assert!(
        denied["tool_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["result"]["code"] == "forbidden")
    );

    // Start another alpha lease with the revised schema, then remove/rename the
    // tool while it is in flight. Its old lease can settle, but alpha disappears
    // from discovery and stays absent even if a separate ACL update regrants it.
    call(
        &core,
        &alice,
        "tools.update_acl",
        acl_payload(
            vec![old_grant.clone()],
            vec![old_grant.clone()],
            vec![old_grant.clone()],
        ),
    )
    .await;
    *response.lock().await = call_response("call-alpha-v2", "alpha", "three");
    let second_lease = call(&core, &alice, "worker.run", json!({})).await;
    let second_task = second_lease["host_tasks"][0]["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        second_lease["host_tasks"][0]["descriptor"]["schema"],
        schema("v2")
    );
    call(
        &core,
        &alice,
        "tools.sync",
        sync_payload(
            vec![tool("beta", "renamed beta", schema("b1"))],
            vec![beta_grant.clone()],
            vec![beta_grant.clone()],
            vec![beta_grant.clone()],
        ),
    )
    .await;
    let after_rename = call(&core, &alice, "tools.status", json!({})).await;
    assert_eq!(after_rename["catalog"].as_array().unwrap().len(), 1);
    assert_eq!(after_rename["catalog"][0]["name"], "beta");
    assert_eq!(after_rename["active_leases"].as_array().unwrap().len(), 1);
    call(
        &core,
        &alice,
        "tool.claim",
        json!({"task_id":second_task,"claim_id":"fixture-claim-alpha-2"}),
    )
    .await;
    let lease_finish=call(&core,&alice,"tool.complete",json!({"task_id":second_task,"claim_id":"fixture-claim-alpha-2","succeeded":true,"result":{"fixture":"finish old alpha lease after rename"}})).await;
    assert_eq!(lease_finish["status"], "completed");
    call(
        &core,
        &alice,
        "tools.update_acl",
        acl_payload(
            vec![beta_grant.clone(), old_grant.clone()],
            vec![beta_grant.clone(), old_grant.clone()],
            vec![beta_grant.clone(), old_grant.clone()],
        ),
    )
    .await;
    let after_acl_regrant = call(&core, &alice, "tools.status", json!({})).await;
    assert_eq!(after_acl_regrant["catalog"].as_array().unwrap().len(), 1);
    assert_eq!(after_acl_regrant["catalog"][0]["name"], "beta");
    *response.lock().await = call_response("call-alpha-gone", "alpha", "four");
    let gone = call(&core, &alice, "worker.run", json!({})).await;
    assert!(gone["host_tasks"].as_array().unwrap().is_empty());
    assert!(
        gone["tool_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["result"]["ok"] == false)
    );
    *response.lock().await = call_response("call-beta", "beta", "five");
    let renamed = call(&core, &alice, "worker.run", json!({})).await;
    assert_eq!(renamed["host_tasks"][0]["descriptor"]["name"], "beta");
    assert_eq!(
        renamed["host_tasks"][0]["descriptor"]["schema"],
        schema("b1")
    );
    let beta_task = renamed["host_tasks"][0]["task_id"].as_str().unwrap();
    call(
        &core,
        &alice,
        "tool.claim",
        json!({"task_id":beta_task,"claim_id":"fixture-claim-beta"}),
    )
    .await;
    call(
        &core,
        &alice,
        "tool.complete",
        json!({"task_id":beta_task,"claim_id":"fixture-claim-beta","succeeded":true,"result":{"fixture":"beta complete"}}),
    )
    .await;

    call(
        &core,
        &alice,
        "tools.sync",
        sync_payload(vec![], vec![], vec![], vec![]),
    )
    .await;
    call(
        &core,
        &alice,
        "tools.update_acl",
        acl_payload(
            vec![beta_grant.clone()],
            vec![beta_grant.clone()],
            vec![beta_grant.clone()],
        ),
    )
    .await;
    assert!(
        call(&core, &alice, "tools.status", json!({})).await["catalog"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    *response.lock().await = call_response("call-beta-removed", "beta", "six");
    let removed = call(&core, &alice, "worker.run", json!({})).await;
    assert!(removed["host_tasks"].as_array().unwrap().is_empty());
    assert!(
        removed["tool_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["result"]["ok"] == false)
    );
    server.abort();
}
