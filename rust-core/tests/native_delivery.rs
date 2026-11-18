use serde_json::json;
use sic_core_rust::{core::{Core,OperationContext},storage::Store};
#[tokio::test]
async fn commit_delivery_survives_transport_retry_and_i_is_adopted_only_in_a_real_readonly_suffix() {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"delivery-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let begin=json!({"format":"openai-responses","request":{"input":"before"},"idempotency_key":"request-one"});
    let source=core.operate("native.begin",&auth,&begin).await.unwrap()["result"].clone();
    let delivery=json!({"ops":[{"op":"COMPRESS","target":"u1","content":"after"}],"pvi":{"V":["later"],"I":{"phase":"prepared"}}});
    let commit=json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,
        "edits":[{"unit_id":"u1","expected_text":"before","replacement":"after"}],"delivery":delivery});
    let mut invalid=commit.clone();
    invalid["delivery"]["ops"][0]["content"]=json!("not the candidate");
    assert_eq!(core.operate("native.commit",&auth,&invalid).await.unwrap_err().status,409);
    let done=core.operate("native.commit",&auth,&commit).await.unwrap();
    assert_eq!(done["result"]["delivery"],delivery);
    assert_eq!(done["result"]["delivery_version"],1);
    let retry=core.operate("native.begin",&auth,&begin).await.unwrap();
    assert_eq!(retry["result"]["delivery"],delivery);
    assert_eq!(retry["result"]["delivery_digest"],done["result"]["delivery_digest"]);
    assert_eq!(core.operate("native.commit",&auth,&commit).await.unwrap()["result"]["delivery"],delivery);
    let mut pubcall=json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,
        "expected_cut":0,"idempotency_key":"publish-one"});
    pubcall["pvi"]=json!({});
    assert_eq!(core.operate("native.publish",&auth,&pubcall).await.unwrap_err().status,409);
    pubcall.as_object_mut().unwrap().remove("pvi");
    let published=core.operate("native.publish",&auth,&pubcall).await.unwrap()["result"]["publication"].clone();
    assert_eq!(published["publication"]["instant_views"]["phase"],"prepared");
    let cut=core.operate("cut.get",&auth,&json!({})).await.unwrap()["result"].clone();
    assert_eq!(cut["instant_views"]["phase"],"prepared");
    let section=format!("sic:instant-views\n{}",cut["instant_view_text"].as_str().unwrap());
    let v=&published["publication"]["next_working_material"]["source_id"];
    let request=json!({"format":"openai-responses",
        "request":{"input":[{"role":"user","content":[{"type":"input_text","text":"after"}]},
            {"role":"user","content":[{"type":"input_text","text":"later"}]},
            {"role":"user","content":[{"type":"input_text","text":"new task"}]},
            {"role":"user","content":[{"type":"input_text","text":section}]}]},
        "editable_unit_ids":["u2","u3"],
        "protected_cb_bindings":[{"cb_id":published["cb_id"],"unit_ids":["u1"]}],
        "volatile_source_bindings":[{"source_id":v,"unit_ids":["u2"]}],
        "adopted_i":{"cut_id":cut["cut_id"],"view_digest":cut["instant_view_digest"],
            "native_anchor":["input",3,"content",0,"text"],"section_text":section}});
    let mut forged=request.clone();
    forged["request"]["input"][3]["content"][0]["text"]=json!("the I section was never sent");
    assert_eq!(core.operate("native.begin",&auth,&forged).await.unwrap_err().status,403);
    let mut forged=request.clone();
    forged["adopted_i"]["view_digest"]=json!("forged");
    assert_eq!(core.operate("native.begin",&auth,&forged).await.unwrap_err().status,409);
    let next=core.operate("native.begin",&auth,&request).await.unwrap()["result"].clone();
    assert_eq!(next["binding"]["adopted_i"]["view_digest"],cut["instant_view_digest"]);
    assert_eq!(next["request"],request["request"]);
    let mut attack=json!({"source_id":next["source_id"],"expected_source_digest":next["source_digest"],"expected_version":0,
        "edits":[{"unit_id":"u4","expected_text":section,"replacement":"forged I"}]});
    assert_eq!(core.operate("native.commit",&auth,&attack).await.unwrap_err().status,403);
    attack["edits"]=json!([]);
    let state=core.operate("native.commit",&auth,&attack).await.unwrap();
    assert_eq!(state["result"]["request"]["input"][3]["content"][0]["text"],section);
}
