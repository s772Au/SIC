use serde_json::json;
use sic_core_rust::{core::{Core,OperationContext},storage::Store};
#[tokio::test]
async fn preview_uses_the_real_materializer_without_persisting_and_commit_publishes_its_delivery() {
 let core=Core::new(Store::memory().await.unwrap());
 let mut a=OperationContext{actor:"preview-owner".into(),allow_write:true,..Default::default()};
 a.chain_id=core.operate("chain.create",&a,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
 let raw=json!({"messages":[{"role":"user","content":[{"type":"text","text":"A"},{"type":"text","text":"B"}]}],"future":{"opaque":true}});
 let s=core.operate("native.begin",&a,&json!({"format":"dsh-messages","request":raw})).await.unwrap()["result"].clone();
 assert_eq!(s["binding"]["edit_capabilities"]["ops"].as_array().unwrap().len(),7);
 assert_eq!(s["binding"]["edit_capabilities"]["pvi"],json!(["P","V","I"]));
 assert_eq!(s["binding"]["edit_capabilities"]["required_host_bindings"]["V"],"native-volatile-next-WB-v1");
 let delivery=json!({"ops":[{"op":"REORDER","target":"u2","order":-1},{"op":"ADD","content":"added"}],"pvi":{"P":["persistent"],"I":{"phase":"previewed"}}});
 let mut q=json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"ops":delivery["ops"],"pvi":delivery["pvi"]});
 let p=core.operate("native.preview",&a,&q).await.unwrap()["result"].clone();
 assert_eq!(p["preview_only"],true);
 assert_eq!(p["material"]["body"],"B\nA\nadded\npersistent");
 assert_eq!(p["request"]["future"],raw["future"]);
 let get=core.operate("native.get",&a,&json!({"source_id":s["source_id"]})).await.unwrap();
 assert_eq!(get["result"]["version"],0);assert_eq!(get["result"]["request"],raw);
 q["expected_version"]=json!(9);
 assert_eq!(core.operate("native.preview",&a,&q).await.unwrap_err().status,409);
 core.operate("native.commit",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"edits":p["edits"],"delivery":delivery})).await.unwrap();
 let published=core.operate("native.publish",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"preview-publish"})).await.unwrap();
 assert_eq!(published["result"]["request"],p["request"]);
 let replay=core.operate("memory.replay",&a,&json!({"cb_id":published["result"]["publication"]["cb_id"]})).await.unwrap();
 assert_eq!(replay["result"]["body"],p["material"]["body"]);
}
