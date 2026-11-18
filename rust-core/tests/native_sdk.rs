use serde_json::json;
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
