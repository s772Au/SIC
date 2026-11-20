use serde_json::{Value, json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store,transparent::NativeContext};
#[tokio::test]
async fn host_sdk_messages_are_edited_without_a_provider_api_conversion() {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"sdk-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let raw=json!({"model":{"opaqueModelRef":"harness-route"},"provider_controls":{"stream":true,"n":7,"future_option":[1,2]},
        "tools":[{"name":"native_tool","schema":{"type":"object"}}],
        "messages":[{"id":"sys","role":"system","source":{"type":"system"},"content":[{"type":"text","text":"protected"}]},
            {"id":"assistant","role":"assistant","source":{"type":"model"},"content":[
                {"type":"text","text":"readonly assistant text"},{"type":"reasoning","text":"opaque reasoning","signature":"opaque-signature"},
                {"type":"tool-call","id":"call-1","name":"native_tool","arguments":{"text":"keep arguments"}},
                {"type":"replay","provider":"future-api","payload":{"signed":"opaque"}}]},
            {"id":"tool-result","role":"tool","toolCallId":"call-1","isError":false,"source":{"type":"tool"},
                "content":[{"type":"text","text":"before"},{"type":"image","data":{"opaque":"media"}}]}]});
    let source=core.operate("native.begin",&auth,&json!({"format":"dsh-messages","request":raw,
        "editable_anchors":[["messages",2,"content",0,"text"]],
        "host_message_origins":[{"message_id":"tool-result","block_index":0,"native_anchor":["messages",2,"content",0,"text"]}]
    })).await.unwrap()["result"].clone();
    assert_eq!(source["binding"]["editable_unit_ids"],json!(["u2"]));
    core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u2","expected_text":"before","replacement":"after"}]})).await.unwrap();
    let done=core.operate("native.publish",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":1,"expected_cut":0,"idempotency_key":"sdk-publish"})).await.unwrap();
    let mut expected=raw.clone();
    expected["messages"][2]["content"][0]["text"]=json!("after");
    assert_eq!(done["result"]["request"],expected);
    assert_eq!(done["result"]["source_request"],raw);
    let replay=core.operate("memory.replay",&auth,&json!({"cb_id":done["result"]["publication"]["cb_id"]})).await.unwrap();
    assert_eq!(replay["result"]["body"],"after");
}
#[tokio::test]
async fn sdk_only_text_blocks_are_targets_and_open_tool_groups_do_not_freeze() {
    let raw=json!({"messages":[{"id":"assistant","role":"assistant","content":[{"type":"text","text":"before"},
        {"type":"tool-call","id":"pending","name":"native_tool","arguments":{"text":"opaque"}}]}]});
    let context=NativeContext::new("dsh-messages",&raw).unwrap();
    assert_eq!(context.units().len(),1);
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"sdk-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let error=core.operate("native.begin",&auth,&json!({"format":"dsh-messages","request":raw})).await.unwrap_err();
    assert_eq!(error.code,"tool_group_open");
    let readonly=core.operate("native.begin",&auth,&json!({"format":"dsh-messages","request":raw,"editable_anchors":[]})).await.unwrap();
    assert_eq!(readonly["result"]["request"],raw);
}

#[tokio::test]
async fn opencode_carrier_rejects_incomplete_or_mismatched_tool_groups() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut auth = OperationContext { actor: "opencode-host".into(), allow_write: true, ..Default::default() };
    let chain = core.operate("chain.create", &auth, &json!({})).await.unwrap();
    auth.chain_id = chain["result"]["chain_id"].as_str().map(str::to_owned);
    let mut payload = json!({
        "format": "opencode-messages",
        "request": { "model": {"providerID":"local","id":"fixture"}, "messages": [
            {"role":"user","content":[{"type":"text","text":"store a native sample"}]},
            {"role":"assistant","content":[{"type":"tool-call","id":"call-1","name":"read","input":{"path":"fixture"}}]}
        ]},
        "editable_anchors":[["messages",0,"content",0,"text"]]
    });
    let error = core.operate("native.begin", &auth, &payload).await.unwrap_err();
    assert_eq!(error.code, "tool_group_open");
    payload["request"]["messages"].as_array_mut().unwrap().push(json!({
        "role":"tool","content":[{"type":"tool-result","id":"call-1","name":"other",
            "result":{"type":"json","value":{"value":14}}}]
    }));
    let error = core.operate("native.begin", &auth, &payload).await.unwrap_err();
    assert_eq!(error.status, 403);
    payload["request"]["messages"][2]["content"][0]["name"] = json!("read");
    let selected = core.operate("native.begin", &auth, &payload).await.unwrap();
    assert_eq!(selected["result"]["source_request"], payload["request"]);
    assert_eq!(selected["result"]["binding"]["editable_unit_ids"], json!(["u1"]));
    assert_eq!(selected["result"]["opaque_blocks"].as_array().unwrap().len(), 2);
}

fn sdk_tool_wait() -> Value {
    json!({"format":"dsh-messages","native_step_id":"a".repeat(64),
        "tool_wait":{"assistant_message_id":"formed","assistant_event_seq":7,"call_ids":["pending"]},
        "editable_anchors":[["messages",0,"content",0,"text"]],
        "host_message_origins":[{"message_id":"formed","block_index":0,"native_anchor":["messages",0,"content",0,"text"]}],
        "request":{"provider":"local","model":"harness-model","messages":[
            {"id":"formed","role":"assistant","source":{"kind":"model","provider":"local","model":"harness-model"},
              "content":[{"type":"text","text":"formed progress"},{"type":"tool-call","id":"pending","name":"native_tool","arguments":"{}"}]},
            {"id":"runtime","role":"user","source":{"kind":"runtime-context"},"content":[{"type":"text","text":"activation"}]}
        ]}})
}

async fn commit_progress(core: &Core, auth: &OperationContext, source: &Value) -> Value {
    core.operate("native.commit", auth, &json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"formed progress","replacement":"retained progress"}]}))
        .await.unwrap();
    json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":1,"expected_cut":0,"idempotency_key":"tool-wait"})
}

#[tokio::test]
async fn authenticated_open_sdk_wb_can_publish_with_original_envelope_and_no_tool_result() {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"sdk-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let input = sdk_tool_wait();
    let mut wrong = input.clone();
    wrong["tool_wait"]["call_ids"] = json!(["unknown"]);
    assert_eq!(core.operate("native.begin",&auth,&wrong).await.unwrap_err().status,403);
    let mut empty = input.clone();
    empty["request"]["messages"][0]["content"][0]["text"] = json!(" \t");
    assert_eq!(core.operate("native.begin",&auth,&empty).await.unwrap_err().status,403);
    let mut late = input.clone();
    late["request"]["messages"].as_array_mut().unwrap().insert(1,json!({
        "id":"late-user","role":"user","source":{"kind":"user"},"content":[{"type":"text","text":"arrived after the freeze"}]
    }));
    assert_eq!(core.operate("native.begin",&auth,&late).await.unwrap_err().status,403);
    let source = core.operate("native.begin",&auth,&input).await.unwrap()["result"].clone();
    assert_eq!(source["binding"]["tool_wait"]["boundary"],input["tool_wait"]);
    let chain = core.operate("chain.get",&auth,&json!({})).await.unwrap();
    assert_eq!(chain["result"]["contexts"]["main"]["levels"]["1"]["pending"],json!([]));
    let publish = commit_progress(&core,&auth,&source).await;
    let receipt = core.operate("native.publish",&auth,&publish).await.unwrap();
    let publication = &receipt["result"]["publication"]["publication"];
    assert_eq!(receipt["result"]["source_request"],input["request"]);
    assert_eq!(publication["tool_wait"]["result_ownership"],"native-host-next-boundary");
    assert_eq!(receipt["result"]["request"]["messages"][0]["content"][1],input["request"]["messages"][0]["content"][1]);
    let archived = core.operate("source.read", &auth, &json!({"source_id":source["binding"]["core_source_id"]})).await.unwrap();
    assert_eq!(archived["result"]["source"]["open_calls"],json!(["pending"]));
    let replay = core.operate("memory.replay",&auth,&json!({"cb_id":publication["cb_id"]})).await.unwrap();
    assert_eq!(replay["result"]["body"],"retained progress");
    assert_eq!(core.operate("native.publish",&auth,&publish).await.unwrap()["evidence"]["replayed"],true);
}

#[tokio::test]
async fn interrupted_sdk_tool_wait_does_not_own_pending_material_or_publish_across_new_input() {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"sdk-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let source = core.operate("native.begin",&auth,&sdk_tool_wait()).await.unwrap()["result"].clone();
    let publish = commit_progress(&core,&auth,&source).await;
    let closed = core.operate("native.begin",&auth,&json!({"format":"dsh-messages",
        "request":{"messages":[{"id":"continued","role":"user","content":[{"type":"text","text":"closed input"}]}]}})).await.unwrap();
    let closed_id = &closed["result"]["binding"]["core_source_id"];
    let conflict = core.operate("native.publish",&auth,&publish).await.unwrap_err();
    assert_eq!(conflict.status,409);
    let chain = core.operate("chain.get",&auth,&json!({})).await.unwrap();
    assert_eq!(chain["result"]["contexts"]["main"]["levels"]["1"]["pending"],json!([closed_id]));
    assert_eq!(chain["result"]["contexts"]["main"]["levels"]["1"]["cbs"],json!([]));
}
