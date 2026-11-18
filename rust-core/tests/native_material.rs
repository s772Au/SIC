use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store};
async fn setup()->(Core,OperationContext) {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"material-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    (core,auth)
}
async fn begin(core:&Core,auth:&OperationContext,format:&str,request:Value)->Value {
    core.operate("native.begin",auth,&json!({"format":format,"request":request})).await.unwrap()["result"].clone()
}
async fn commit(core:&Core,auth:&OperationContext,source:&Value,edits:Value) {
    core.operate("native.commit",auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,"edits":edits})).await.unwrap();
}
fn publish(source:&Value,cut:u64,key:&str,ops:Value,pvi:Value)->Value {
    json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,
        "expected_cut":cut,"idempotency_key":key,"ops":ops,"pvi":pvi})
}
#[tokio::test]
async fn add_and_persisted_content_splice_real_native_text_and_replay_existing_operations() {
    let fixtures=[
        ("openai-chat",json!({"messages":[{"role":"user","content":"before"}]})),
        ("openai-responses",json!({"input":"before","opaque":{"future":"preserved"}})),
        ("anthropic-messages",json!({"messages":[{"role":"user","content":[{"type":"text","text":"before"}]}]})),
    ];
    for (format,raw) in fixtures {
        let (core,auth)=setup().await;
        let source=begin(&core,&auth,format,raw).await;
        let result="before\nanchored\nunanchored\npersistent";
        commit(&core,&auth,&source,json!([{"unit_id":"u1","expected_text":"before","replacement":result}])).await;
        let done=core.operate("native.publish",&auth,&publish(&source,0,"adds",
            json!([{"op":"ADD","content":"anchored","anchor":{"target":"u1","position":"after"}},
                {"op":"ADD","content":"unanchored"}]),json!({"P":["persistent"]}))).await.unwrap();
        assert_eq!(done["result"]["units"][0]["text"],result);
        let replay=core.operate("memory.replay",&auth,&json!({"cb_id":done["result"]["publication"]["cb_id"]})).await.unwrap();
        assert_eq!(replay["result"]["body"],result);
        assert_eq!(replay["result"]["body_matches_published_cb"],true);
    }
}
#[tokio::test]
async fn reorder_maps_only_a_pure_sibling_text_segment_and_rejects_cross_role_or_opaque_moves() {
    let fixtures=[
        ("openai-chat",json!({"messages":[{"role":"user","content":[{"type":"text","text":"A"},{"type":"text","text":"B"}]}]}),true),
        ("openai-responses",json!({"input":[{"role":"user","content":[{"type":"input_text","text":"A"},{"type":"input_text","text":"B"}]}]}),true),
        ("anthropic-messages",json!({"messages":[{"role":"user","content":[{"type":"text","text":"A"},{"type":"text","text":"B"}]}]}),true),
        ("openai-chat",json!({"messages":[{"role":"user","content":"A"},{"role":"assistant","content":"B"}]}),false),
        ("anthropic-messages",json!({"messages":[{"role":"user","content":[{"type":"text","text":"A"},{"type":"image","opaque":"preserved"},{"type":"text","text":"B"}]}]}),false),
    ];
    for (format,raw,allowed) in fixtures {
        let (core,auth)=setup().await;
        let source=begin(&core,&auth,format,raw).await;
        commit(&core,&auth,&source,json!([{"unit_id":"u1","expected_text":"A","replacement":"B"},{"unit_id":"u2","expected_text":"B","replacement":"A"}])).await;
        let done=core.operate("native.publish",&auth,&publish(&source,0,"reorder",json!([{"op":"REORDER","target":"u2","order":-1}]),json!({}))).await;
        if allowed {
            let done=done.unwrap();
            assert_eq!(done["result"]["units"][0]["text"],"B");
            assert_eq!(done["result"]["publication"]["publication"]["native_leaf_projection"][0]["source_unit_id"],"u2");
            let replay=core.operate("memory.replay",&auth,&json!({"cb_id":done["result"]["publication"]["cb_id"]})).await.unwrap();
            assert_eq!(replay["result"]["body"],"B\nA");
        } else {assert_eq!(done.unwrap_err().status,501);}
    }
}
#[tokio::test]
async fn volatile_output_is_a_real_next_wb_after_host_native_append_and_binding() {
    let (core,auth)=setup().await;
    let source=begin(&core,&auth,"openai-responses",json!({"input":"first"})).await;
    commit(&core,&auth,&source,json!([])).await;
    let done=core.operate("native.publish",&auth,&publish(&source,0,"volatile",json!([]),json!({"V":["later"]}))).await.unwrap();
    assert_eq!(done["result"]["request"]["input"],"first");
    let published=&done["result"]["publication"]["publication"];
    let next=&published["next_working_material"];
    let volatile_id=&next["source_id"];
    assert_eq!(next["units"][0]["text"],"later");
    let raw=json!({"stream":true,"input":[{"role":"user","content":[{"type":"input_text","text":"first"}]},
        {"role":"user","content":[{"type":"input_text","text":"later"}]},{"role":"user","content":[{"type":"input_text","text":"new"}]}]});
    let mut second=json!({"format":"openai-responses","request":raw,"editable_unit_ids":["u2","u3"],
        "protected_cb_bindings":[{"cb_id":published["cb_id"],"unit_ids":["u1"]}]});
    assert_eq!(core.operate("native.begin",&auth,&second).await.unwrap_err().status,409);
    second["volatile_source_bindings"]=json!([{"source_id":volatile_id,"native_anchors":[["input",1,"content",0,"text"]]}]);
    let second=core.operate("native.begin",&auth,&second).await.unwrap()["result"].clone();
    let state=core.operate("chain.get",&auth,&json!({})).await.unwrap();
    assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["pending"].as_array().unwrap().len(),1);
    commit(&core,&auth,&second,json!([])).await;
    let adopted=core.operate("native.publish",&auth,&publish(&second,1,"next-wb",json!([]),json!({}))).await.unwrap();
    let replay=core.operate("memory.replay",&auth,&json!({"cb_id":adopted["result"]["publication"]["cb_id"]})).await.unwrap();
    assert_eq!(replay["result"]["body"],"later\nnew");
    let archived=core.operate("source.read",&auth,&json!({"source_id":second["binding"]["core_source_id"]})).await.unwrap();
    assert_eq!(archived["result"]["source"]["dependencies"],json!([volatile_id]));
    assert_eq!(archived["result"]["source"]["units"][0]["anchor"]["volatile_origin"]["source_id"],*volatile_id);
}

#[tokio::test]
async fn deleted_add_anchors_are_rejected_by_the_actual_materializer_before_publication() {
    for ops in [
        json!([{"op":"DELETE","target":"u1"},{"op":"ADD","content":"new","anchor":{"target":"u1","position":"after"}}]),
        json!([{"op":"ADD","content":"new","anchor":{"target":"u1","position":"before"}},{"op":"DELETE","target":"u1"}]),
    ] {
        let (core,auth)=setup().await;
        let source=begin(&core,&auth,"openai-responses",json!({"input":"A"})).await;
        commit(&core,&auth,&source,json!([{"unit_id":"u1","expected_text":"A","replacement":""}])).await;
        let error=core.operate("native.publish",&auth,&publish(&source,0,"deleted-anchor",ops,json!({}))).await.unwrap_err();
        assert_eq!(error.status,422);
        assert!(error.message.contains("ADD anchor was deleted"));
        let state=core.operate("chain.get",&auth,&json!({})).await.unwrap();
        assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["wbs"],json!([]));
        assert_eq!(state["result"]["contexts"]["main"]["levels"]["1"]["cbs"],json!([]));
    }
}

#[tokio::test]
async fn combined_edit_purposes_have_no_content_loss_between_native_projection_cb_and_replay() {
    let cases=[
        ("A","", "B\nnew","B\nnew",
            json!([{"op":"DELETE","target":"u1"},{"op":"ADD","content":"new"}]),json!({})),
        ("A","", "new","new",
            json!([{"op":"DELETE","target":"u1"},{"op":"DELETE","target":"u2"},{"op":"ADD","content":"new"}]),json!({})),
        ("","x\n","B","x\n\nB",
            json!([{"op":"ADD","content":"x","anchor":{"target":"u1","position":"before"}}]),json!({})),
        ("A","","B","B",
            json!([{"op":"FLAG","target":"u1","content":"note"},{"op":"DELETE","target":"u1"}]),json!({})),
        ("A","B\ntail","A","B\ntail\nA",
            json!([{"op":"REORDER","target":"u2","order":-1},{"op":"ADD","content":"tail","anchor":{"target":"u2","position":"after"}}]),json!({})),
        ("A","\n[FLAG] note","prefix\nB","\n[FLAG] note\nprefix\nB",
            json!([{"op":"REPLACE","target":"u1","content":""},{"op":"FLAG","target":"u1","content":"note"},
                {"op":"ADD","content":"prefix","anchor":{"target":"u2","position":"before"}}]),json!({})),
        ("A","","B\nnew","B\nnew",
            json!([{"op":"REPLACE","target":"u1","content":"later","lifecycle":"V"},{"op":"ADD","content":"new"}]),json!({})),
        ("A","new\ntail","B","new\ntail\nB",
            json!([{"op":"DELETE","target":"u1"},{"op":"REPLACE","target":"u1","content":"new"},
                {"op":"ADD","content":"tail","anchor":{"target":"u1","position":"after"}}]),json!({})),
        ("A","C\n[FLAG] a; b","B","C\n[FLAG] a; b\nB",
            json!([{"op":"FLAG","target":"u1","content":"a"},{"op":"FLAG","target":"u1","content":"b"},{"op":"REPLACE","target":"u1","content":"C"}]),json!({})),
    ];
    for (before,one,two,body,ops,pvi) in cases {
        let (core,auth)=setup().await;
        let raw=json!({"stream":true,"opaque":{"unchanged":true},"messages":[{"role":"user","content":[
            {"type":"text","text":before},{"type":"text","text":"B"}]}]});
        let source=begin(&core,&auth,"openai-chat",raw.clone()).await;
        commit(&core,&auth,&source,json!([{"unit_id":"u1","expected_text":before,"replacement":one},
            {"unit_id":"u2","expected_text":"B","replacement":two}])).await;
        let done=core.operate("native.publish",&auth,&publish(&source,0,"combined",ops,pvi)).await.unwrap();
        let mut expected=raw;
        expected["messages"][0]["content"][0]["text"]=json!(one);
        expected["messages"][0]["content"][1]["text"]=json!(two);
        assert_eq!(done["result"]["request"],expected);
        let replay=core.operate("memory.replay",&auth,&json!({"cb_id":done["result"]["publication"]["cb_id"]})).await.unwrap();
        assert_eq!(replay["result"]["body"],body);
        assert_eq!(replay["result"]["body_matches_published_cb"],true);
    }
}
