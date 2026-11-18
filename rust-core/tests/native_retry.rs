use serde_json::json;
use sic_core_rust::{core::{Core,OperationContext},storage::Store};
#[tokio::test]
async fn request_retries_reuse_native_sources_commits_and_publications_with_raw_wire_identity() {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"retry-owner".into(),allow_write:true,allow_revoke:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let request=json!({"format":"openai-responses","request":{"stream":true,"input":"before","opaque":1},
        "idempotency_key":"real-host-request-1","wire_sha256":"a".repeat(64)});
    let (a,b)=tokio::join!(core.operate("native.begin",&auth,&request),core.operate("native.begin",&auth,&request));
    let (a,b)=(a.unwrap(),b.unwrap());
    assert_eq!(a["result"]["source_id"],b["result"]["source_id"]);
    assert_ne!(a["evidence"]["replayed"],b["evidence"]["replayed"]);
    let source=&a["result"];
    for change in [json!({"wire_sha256":"b".repeat(64)}),json!({"editable_unit_ids":[]})] {
        let mut different=request.clone();
        for (key,value) in change.as_object().unwrap() { different[key]=value.clone(); }
        assert_eq!(core.operate("native.begin",&auth,&different).await.unwrap_err().status,409);
    }
    let edit=json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"idempotency_key":"host-edit-1","edits":[{"unit_id":"u1","expected_text":"before","replacement":"after"}]});
    let first=core.operate("native.commit",&auth,&edit).await.unwrap();
    let repeated=core.operate("native.commit",&auth,&edit).await.unwrap();
    assert_eq!(first["result"],repeated["result"]);
    assert_eq!(repeated["evidence"]["replayed"],true);
    assert_eq!(repeated["evidence"]["replayed_commit_version"],1);
    let mut different=edit.clone();
    different["edits"][0]["replacement"]=json!("forged");
    assert_eq!(core.operate("native.commit",&auth,&different).await.unwrap_err().status,409);
    let published=core.operate("native.publish",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":1,"expected_cut":0,"idempotency_key":"host-publish-1"})).await.unwrap();
    let retry=core.operate("native.begin",&auth,&request).await.unwrap();
    assert_eq!(retry["operation"],"native.begin");
    assert_eq!(retry["evidence"]["replayed"],true);
    assert_eq!(retry["result"],published["result"]);
    assert_eq!(retry["result"]["request"]["input"],"after");
    let repeated=core.operate("native.commit",&auth,&edit).await.unwrap();
    assert_eq!(repeated["result"],published["result"]);
    let state=core.operate("chain.get",&auth,&json!({})).await.unwrap();
    let level=&state["result"]["contexts"]["main"]["levels"]["1"];
    assert_eq!(level["cbs"].as_array().unwrap().len(),1);
    assert_eq!(level["wbs"].as_array().unwrap().len(),1);
    assert_eq!(level["pending"],json!([]));
    core.operate("security.revoke",&auth,&json!({"source_id":source["binding"]["core_source_id"]})).await.unwrap();
    assert_eq!(core.operate("native.begin",&auth,&request).await.unwrap_err().status,403);
    assert_eq!(core.operate("native.commit",&auth,&edit).await.unwrap_err().status,403);
}
