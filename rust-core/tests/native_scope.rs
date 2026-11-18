use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store,tool_dispatch::{AuthenticatedEditorScope,dispatch_editor_native_tool},transparent::TextEdit};
async fn chain(core: &Core) -> OperationContext {
    let mut auth = OperationContext {actor:"owner".into(),allow_write:true,allow_revoke:true,..Default::default()};
    auth.chain_id = core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    auth
}
async fn edit(core: &Core,auth: &OperationContext,source: &Value,id: &str,before: &str,after: &str) {
    core.operate("native.commit",auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[{"unit_id":id,"expected_text":before,"replacement":after}]})).await.unwrap();
}
fn publish(source:&Value,cut:u64,key:&str) -> Value {
    json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,
        "expected_cut":cut,"idempotency_key":key})
}
#[tokio::test]
async fn repeated_native_history_is_readonly_and_only_new_anchors_enter_the_next_wb() {
    let fixtures = [
        ("openai-chat",json!({"messages":[{"role":"user","content":"before"}]}),
            json!({"stream":true,"messages":[{"role":"user","content":"after"},{"role":"user","content":"next"}]}),
            json!(["messages",0,"content"]),json!(["messages",1,"content"])),
        ("openai-responses",json!({"input":"before"}),
            json!({"stream":true,"input":[{"role":"user","content":[{"type":"input_text","text":"after"}]},
                {"role":"user","content":[{"type":"input_text","text":"next"}]}]}),
            json!(["input",0,"content",0,"text"]),json!(["input",1,"content",0,"text"])),
        ("anthropic-messages",json!({"messages":[{"role":"user","content":[{"type":"text","text":"before"}]}]}),
            json!({"stream":true,"messages":[{"role":"user","content":[{"type":"text","text":"after"}]},
                {"role":"user","content":[{"type":"text","text":"next"}]}]}),
            json!(["messages",0,"content",0,"text"]),json!(["messages",1,"content",0,"text"])),
    ];
    for (format,first,next,old_anchor,new_anchor) in fixtures {
        let core = Core::new(Store::memory().await.unwrap());
        let auth = chain(&core).await;
        let one = core.operate("native.begin",&auth,&json!({"format":format,"request":first})).await.unwrap()["result"].clone();
        edit(&core,&auth,&one,"u1","before","after").await;
        let cb1 = core.operate("native.publish",&auth,&publish(&one,0,"first")).await.unwrap()["result"]["publication"]["cb_id"].clone();
        assert_eq!(core.operate("native.begin",&auth,&json!({"format":format,"request":next})).await.unwrap_err().status,403);
        let request = json!({"format":format,"request":next,"editable_anchors":[new_anchor],
            "protected_cb_bindings":[{"cb_id":cb1,"native_anchors":[old_anchor]}]});
        let mut overlap = request.clone();
        overlap["editable_unit_ids"] = json!(["u1","u2"]);
        overlap.as_object_mut().unwrap().remove("editable_anchors");
        assert_eq!(core.operate("native.begin",&auth,&overlap).await.unwrap_err().status,403);
        let mut forged = request.clone();
        if format == "openai-chat" { forged["request"]["messages"][0]["content"] = json!("rewritten old CB"); }
        else if format == "openai-responses" { forged["request"]["input"][0]["content"][0]["text"] = json!("rewritten old CB"); }
        else { forged["request"]["messages"][0]["content"][0]["text"] = json!("rewritten old CB"); }
        assert_eq!(core.operate("native.begin",&auth,&forged).await.unwrap_err().status,409);
        let two = core.operate("native.begin",&auth,&request).await.unwrap()["result"].clone();
        assert_eq!(two["binding"]["editable_unit_ids"],json!(["u2"]));
        let scope = AuthenticatedEditorScope::from_authenticated_host(&auth,two["source_id"].as_str().unwrap(),
            two["source_digest"].as_str().unwrap(),0,vec!["u1".into(),"u2".into()],vec!["apply_patch".into()]).unwrap();
        let denied = dispatch_editor_native_tool(&core,&auth,&scope,"apply_patch",&json!({}),|_,_|Ok(vec![
            TextEdit{unit_id:"u1".into(),expected_text:"after".into(),replacement:"forged".into()}
        ])).await.unwrap_err();
        assert_eq!(denied.status,403);
        edit(&core,&auth,&two,"u2","next","next-after").await;
        let done = core.operate("native.publish",&auth,&publish(&two,1,"second")).await.unwrap();
        let replay = core.operate("memory.replay",&auth,&json!({"cb_id":done["result"]["publication"]["cb_id"]})).await.unwrap();
        assert_eq!(replay["result"]["body"],"next-after");
        let source = core.operate("source.read",&auth,&json!({"source_id":two["binding"]["core_source_id"]})).await.unwrap();
        assert_eq!(source["result"]["source"]["units"].as_array().unwrap().len(),1);
        assert_eq!(source["result"]["source"]["units"][0]["unit_id"],"u2");
        assert_eq!(source["result"]["source"]["sidecar"],next);
        let old = core.operate("memory.replay",&auth,&json!({"cb_id":cb1})).await.unwrap();
        assert_eq!(old["result"]["body"],"after");
        core.operate("security.revoke",&auth,&json!({"source_id":one["binding"]["core_source_id"]})).await.unwrap();
        assert_eq!(core.operate("native.get",&auth,&json!({"source_id":two["source_id"]})).await.unwrap_err().status,403);
    }
}
#[tokio::test]
async fn readonly_scope_does_not_append_a_working_source() {
    let core = Core::new(Store::memory().await.unwrap());
    let auth = chain(&core).await;
    let source = core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"observe"},"editable_anchors":[]})).await.unwrap()["result"].clone();
    assert_eq!(source["binding"]["core_source_id"],Value::Null);
    assert_eq!(core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"observe","replacement":"mutate"}]})).await.unwrap_err().status,403);
    let state = core.operate("chain.get",&auth,&json!({})).await.unwrap();
    assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["pending"],json!([]));
}

#[tokio::test]
async fn trusted_sdk_origins_are_bijective_and_persist_on_only_new_wb_units() {
    let core=Core::new(Store::memory().await.unwrap());
    let auth=chain(&core).await;
    let request=json!({"format":"openai-chat","request":{"messages":[{"role":"user","content":"readonly"},{"role":"user","content":"new"}]},
        "editable_unit_ids":["u2"],"host_message_origins":[
            {"message_id":"old-sdk","block_index":0,"native_anchor":["messages",0,"content"]},
            {"message_id":"new-sdk","block_index":3,"native_anchor":["messages",1,"content"]}]});
    for mode in ["same-origin","same-leaf","unknown-path"] {
        let mut bad=request.clone();
        if mode=="same-origin" {
            bad["host_message_origins"][1]["message_id"]=json!("old-sdk");
            bad["host_message_origins"][1]["block_index"]=json!(0);
        } else if mode=="same-leaf" {
            bad["host_message_origins"][1]["native_anchor"]=json!(["messages",0,"content"]);
        } else {bad["host_message_origins"][1]["native_anchor"]=json!(["messages",99,"content"]);}
        assert_eq!(core.operate("native.begin",&auth,&bad).await.unwrap_err().status,403);
    }
    let source=core.operate("native.begin",&auth,&request).await.unwrap()["result"].clone();
    assert_eq!(source["binding"]["host_message_origins"].as_array().unwrap().len(),2);
    let archived=core.operate("source.read",&auth,&json!({"source_id":source["binding"]["core_source_id"]})).await.unwrap();
    assert_eq!(archived["result"]["source"]["units"].as_array().unwrap().len(),1);
    assert_eq!(archived["result"]["source"]["units"][0]["anchor"]["host_origin"]["message_id"],"new-sdk");
    assert_eq!(archived["result"]["source"]["units"][0]["anchor"]["host_origin"]["block_index"],3);
}
