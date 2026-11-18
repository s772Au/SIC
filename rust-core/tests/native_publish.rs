use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store};

fn auth() -> OperationContext {
    OperationContext {actor:"native-owner".into(),allow_write:true,allow_revoke:true,..Default::default()}
}
async fn chain(core: &Core) -> OperationContext {
    let mut auth = auth();
    auth.chain_id = core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    auth
}
fn publish(source: &Value) -> Value {
    json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":1,"expected_cut":0,"idempotency_key":"native-publish-1",
        "ops":[{"op":"COMPRESS","target":"u1","content":"after","reason":"retain the corrected fact"}],
        "pvi":{"I":{"native-status":"verified candidate"}}})
}
async fn begin(core: &Core,auth: &OperationContext,format: &str,request: Value) -> Value {
    core.operate("native.begin",auth,&json!({"format":format,"request":request})).await.unwrap()["result"].clone()
}
async fn edit(core: &Core,auth: &OperationContext,source: &Value) {
    core.operate("native.commit",auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"before","replacement":"after"}]})).await.unwrap();
}

#[tokio::test]
async fn three_native_protocols_publish_existing_dual_chain_and_replay_without_sidecar_loss() {
    let fixtures = [
        ("openai-chat",json!({"model":"host-chat","stream":true,"n":3,"vendor":{"future":1},
            "messages":[{"role":"system","content":"protected"},{"role":"user","content":"before"}]})),
        ("openai-responses",json!({"model":"host-responses","stream":true,"input":"before",
            "previous_response_id":"opaque-prior","reasoning":{"effort":"low"},"vendor":[1,2]})),
        ("anthropic-messages",json!({"model":"host-anthropic","max_tokens":123,"stream":true,"system":"protected",
            "messages":[{"role":"user","content":[{"type":"text","text":"before"},{"type":"future-media","opaque":[1,2,3]}]}]})),
    ];
    for (format,request) in fixtures {
        let core = Core::new(Store::memory().await.unwrap());
        let auth = chain(&core).await;
        let source = begin(&core,&auth,format,request.clone()).await;
        edit(&core,&auth,&source).await;
        let payload = publish(&source);
        let done = core.operate("native.publish",&auth,&payload).await.unwrap();
        assert_eq!(done["evidence"]["provider_called"],false);
        let adopted = &done["result"];
        assert_eq!(adopted["source_request"],request);
        let mut expected = request.clone();
        match format {
            "openai-chat" => expected["messages"][1]["content"] = json!("after"),
            "openai-responses" => expected["input"] = json!("after"),
            _ => expected["messages"][0]["content"][0]["text"] = json!("after"),
        }
        assert_eq!(adopted["request"],expected);
        let publication = &adopted["publication"]["publication"];
        let archived = core.operate("source.read",&auth,&json!({"source_id":publication["core_source_id"]})).await.unwrap();
        assert_eq!(archived["result"]["source"]["sidecar"],request);
        assert_eq!(archived["result"]["source"]["format"],format);
        let state = core.operate("chain.get",&auth,&json!({})).await.unwrap();
        let level = &state["result"]["contexts"]["main"]["levels"]["1"];
        assert_eq!(level["wbs"].as_array().unwrap().len(),1);
        assert_eq!(level["cbs"].as_array().unwrap().len(),1);
        let wb = publication["wb_id"].as_str().unwrap();
        assert_eq!(level["pairs"][wb],publication["cb_id"]);
        let replay = core.operate("memory.replay",&auth,&json!({"cb_id":publication["cb_id"]})).await.unwrap();
        assert_eq!(replay["result"]["body"],"after");
        assert_eq!(replay["result"]["body_matches_published_cb"],true);
        let again = core.operate("native.publish",&auth,&payload).await.unwrap();
        assert_eq!(again["evidence"]["replayed"],true);
        assert_eq!(again["result"],done["result"]);
        assert_eq!(core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],
            "expected_source_digest":source["source_digest"],"expected_version":1,"edits":[]})).await.unwrap_err().status,409);
        let got = core.operate("native.get",&auth,&json!({"source_id":source["source_id"]})).await.unwrap();
        assert_eq!(got["result"],done["result"]);
    }
}

#[tokio::test]
async fn publication_rejects_stale_mismatched_cross_scope_and_revoked_dependencies() {
    let core = Core::new(Store::memory().await.unwrap());
    let owner = chain(&core).await;
    let other = chain(&core).await;
    let source = begin(&core,&owner,"openai-responses",json!({"model":"host","input":"before"})).await;
    edit(&core,&owner,&source).await;
    let payload = publish(&source);
    for key in ["expected_cut","expected_version"] {
        let mut stale = payload.clone();
        stale[key] = json!(99);
        assert_eq!(core.operate("native.publish",&owner,&stale).await.unwrap_err().status,409);
    }
    let mut mismatch = payload.clone();
    mismatch["ops"][0]["content"] = json!("unadopted");
    assert_eq!(core.operate("native.publish",&owner,&mismatch).await.unwrap_err().status,409);
    let state = core.operate("chain.get",&owner,&json!({})).await.unwrap();
    assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["wbs"],json!([]));
    for caller in [other,OperationContext{actor:"different-owner".into(),..owner.clone()}] {
        assert_eq!(core.operate("native.publish",&caller,&payload).await.unwrap_err().status,403);
        assert_eq!(core.operate("native.get",&caller,&json!({"source_id":source["source_id"]})).await.unwrap_err().status,403);
    }
    let published = core.operate("native.publish",&owner,&payload).await.unwrap();
    let link = &published["result"]["publication"]["publication"];
    let mut different = payload.clone();
    different["idempotency_key"] = json!("different-transaction");
    assert_eq!(core.operate("native.publish",&owner,&different).await.unwrap_err().status,409);
    core.operate("security.revoke",&owner,&json!({"source_id":link["core_source_id"]})).await.unwrap();
    assert_eq!(core.operate("native.get",&owner,&json!({"source_id":source["source_id"]})).await.unwrap_err().status,403);
    assert_eq!(core.operate("native.publish",&owner,&payload).await.unwrap_err().status,403);
    assert_eq!(core.operate("memory.replay",&owner,&json!({"cb_id":link["cb_id"]})).await.unwrap_err().status,403);
}

#[tokio::test]
async fn unbound_candidates_and_unprojected_structure_cannot_claim_a_published_cb() {
    let core = Core::new(Store::memory().await.unwrap());
    let bound = chain(&core).await;
    let unbound = auth();
    let source = begin(&core,&unbound,"openai-responses",json!({"input":"before"})).await;
    edit(&core,&unbound,&source).await;
    assert_eq!(core.operate("native.publish",&unbound,&publish(&source)).await.unwrap_err().status,409);
    assert_eq!(core.operate("native.publish",&bound,&publish(&source)).await.unwrap_err().status,403);
    let source = begin(&core,&bound,"openai-responses",json!({"input":"before"})).await;
    edit(&core,&bound,&source).await;
    for pvi in [json!({"P":["unprojected"]})] {
        let mut payload = publish(&source);
        payload["pvi"] = pvi;
        assert_eq!(core.operate("native.publish",&bound,&payload).await.unwrap_err().status,409);
    }
    let mut payload = publish(&source);
    payload["ops"] = json!([{"op":"ADD","content":"unprojected"}]);
    assert_eq!(core.operate("native.publish",&bound,&payload).await.unwrap_err().status,409);
}

#[tokio::test]
async fn delete_blanks_only_the_authorized_leaf_and_flag_is_an_auditable_text_edit() {
    for (kind,replacement) in [("DELETE",""),("FLAG","before\n[FLAG] review")] {
        let core = Core::new(Store::memory().await.unwrap());
        let auth = chain(&core).await;
        let raw = json!({"stream":true,"messages":[{"role":"user","content":"before"},{"role":"assistant","content":"retained"}]});
        let source = begin(&core,&auth,"openai-chat",raw.clone()).await;
        core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
            "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"before","replacement":replacement}]})).await.unwrap();
        let mut payload = publish(&source);
        payload["ops"] = if kind == "DELETE" { json!([{"op":"DELETE","target":"u1"}]) }
            else { json!([{"op":"FLAG","target":"u1","content":"review"}]) };
        let done = core.operate("native.publish",&auth,&payload).await.unwrap();
        let mut expected = raw.clone();
        expected["messages"][0]["content"] = json!(replacement);
        assert_eq!(done["result"]["request"],expected);
        assert_eq!(done["result"]["source_request"],raw);
        let cb = &done["result"]["publication"]["cb_id"];
        let replay = core.operate("memory.replay",&auth,&json!({"cb_id":cb})).await.unwrap();
        assert_eq!(replay["result"]["body"], if kind == "DELETE" {"retained".into()} else {format!("{replacement}\nretained")});
        assert_eq!(replay["result"]["body_matches_published_cb"],true);
    }
}

#[tokio::test]
async fn unresolved_native_tool_groups_are_not_frozen_as_published_cbs() {
    let fixtures = [
        ("openai-chat",json!({"messages":[{"role":"user","content":"before"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"call-a","type":"function","function":{"name":"original","arguments":"{}"}}]}]})),
        ("openai-responses",json!({"input":[{"role":"user","content":[{"type":"input_text","text":"before"}]},
            {"type":"function_call","call_id":"call-a","name":"original","arguments":"{}"}]})),
        ("anthropic-messages",json!({"messages":[{"role":"user","content":"before"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call-a","name":"original","input":{}}]}]})),
    ];
    for (format,raw) in fixtures {
        let core = Core::new(Store::memory().await.unwrap());
        let auth = chain(&core).await;
        let error = core.operate("native.begin",&auth,&json!({"format":format,"request":raw})).await.unwrap_err();
        assert_eq!(error.code,"tool_group_open");
        let state = core.operate("chain.get",&auth,&json!({})).await.unwrap();
        assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["wbs"],json!([]));
        assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["pending"],json!([]));
    }
}

#[tokio::test]
async fn a_committed_publication_survives_reopen_and_concurrent_publish_is_idempotent() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("native-publish-fixture.db");
    let database = Store::create(&path).await.unwrap();
    let core = Core::new(database.clone());
    let owner = chain(&core).await;
    let source = begin(&core,&owner,"openai-responses",json!({"model":"original","stream":true,"input":"before"})).await;
    edit(&core,&owner,&source).await;
    database.close().await;
    drop(core);
    let one = Store::open(&path).await.unwrap();
    let two = Store::open(&path).await.unwrap();
    let a = Core::new(one.clone());
    let b = Core::new(two.clone());
    let payload = publish(&source);
    let (a,b) = tokio::join!(a.operate("native.publish",&owner,&payload),b.operate("native.publish",&owner,&payload));
    let (a,b) = (a.unwrap(),b.unwrap());
    assert_eq!(a["result"],b["result"]);
    assert_ne!(a["evidence"]["replayed"],b["evidence"]["replayed"]);
    one.close().await;
    two.close().await;
    let reopened = Store::open(&path).await.unwrap();
    let core = Core::new(reopened.clone());
    let got = core.operate("native.get",&owner,&json!({"source_id":source["source_id"]})).await.unwrap();
    assert_eq!(got["result"],a["result"]);
    let state = core.operate("chain.get",&owner,&json!({})).await.unwrap();
    assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["cbs"].as_array().unwrap().len(),1);
    reopened.close().await;
}
