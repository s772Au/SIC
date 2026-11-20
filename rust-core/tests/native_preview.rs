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
 q["expected_version"]=json!(0);
 q["pvi"]["I"]["decision_tree"]=json!({"views":{"Working on":[]},"revision":1});
 if cfg!(feature = "decision-tree") {
  let invalid=core.operate("native.preview",&a,&q).await.unwrap_err();
  assert_eq!(invalid.status,422);
  assert_eq!(invalid.message,"unknown decision_tree field");
 } else {
  let disabled=core.operate("native.preview",&a,&q).await.unwrap();
  assert!(disabled["result"]["material"]["instant"].get("decision_tree").is_none());
 }
 let after=core.operate("native.get",&a,&json!({"source_id":s["source_id"]})).await.unwrap();
 assert_eq!(after["result"]["version"],0);
 core.operate("native.commit",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"edits":p["edits"],"delivery":delivery})).await.unwrap();
 let published=core.operate("native.publish",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"preview-publish"})).await.unwrap();
 assert_eq!(published["result"]["request"],p["request"]);
 let replay=core.operate("memory.replay",&a,&json!({"cb_id":published["result"]["publication"]["cb_id"]})).await.unwrap();
 assert_eq!(replay["result"]["body"],p["material"]["body"]);
}

#[tokio::test]
async fn native_release_retires_only_the_exact_unpublished_pending_step() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut auth = OperationContext { actor: "native-release-owner".into(), allow_write: true, ..Default::default() };
    let chain = core.operate("chain.create", &auth, &json!({})).await.unwrap();
    auth.chain_id = chain["result"]["chain_id"].as_str().map(str::to_owned);
    let step = "a".repeat(64);
    let mut request = json!({"format":"dsh-messages","request":{"messages":[
        {"role":"user","id":"one","content":[{"type":"text","text":"native input"}]}
    ]},"editable_unit_ids":["u1"],"native_step_id":step,"idempotency_key":"first-native-input"});
    let first = core.operate("native.begin", &auth, &request).await.unwrap()["result"].clone();
    let original_request = request.clone();
    request["idempotency_key"] = json!("second-native-input");
    request["request"]["messages"][0]["id"] = json!("two");
    let second = core.operate("native.begin", &auth, &request).await.unwrap()["result"].clone();
    let edit = json!({"source_id":second["source_id"],"expected_source_digest":second["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"native input","replacement":"unpublished edit"}]});
    core.operate("native.commit", &auth, &edit).await.unwrap();
    let proof = json!({"native_step_id":step,"sources":[
        {"source_id":first["source_id"],"expected_source_digest":first["source_digest"],"expected_version":0},
        {"source_id":second["source_id"],"expected_source_digest":second["source_digest"],"expected_version":1}
    ]});
    let mut partial = proof.clone();
    partial["sources"].as_array_mut().unwrap().remove(0);
    assert_eq!(core.operate("native.abandon", &auth, &partial).await.unwrap_err().status, 409);
    let result = core.operate("native.abandon", &auth, &proof).await.unwrap();
    let replay = core.operate("native.abandon", &auth, &proof).await.unwrap();
    assert_eq!(replay["result"], result["result"]);
    assert_eq!(replay["evidence"]["replayed"], true);
    assert_eq!(core.operate("native.begin", &auth, &original_request).await.unwrap_err().status, 409);
    let archived = core.operate("native.get", &auth, &json!({"source_id":second["source_id"]})).await.unwrap();
    assert_eq!(archived["result"]["version"], 1);
    assert!(archived["result"]["publication"].is_null());
    request["idempotency_key"] = json!("third-native-input");
    let final_candidate = core.operate("native.begin", &auth, &request).await.unwrap()["result"].clone();
    let new_edit = json!({"source_id":final_candidate["source_id"],"expected_source_digest":final_candidate["source_digest"],
        "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"native input","replacement":"adopted edit"}]});
    core.operate("native.commit", &auth, &new_edit).await.unwrap();
    let published = core.operate("native.publish", &auth, &json!({"source_id":final_candidate["source_id"],
        "expected_source_digest":final_candidate["source_digest"],"expected_version":1,"expected_cut":0,
        "idempotency_key":"new-native-publish"})).await.unwrap();
    let replay = core.operate("memory.replay", &auth, &json!({"cb_id":published["result"]["publication"]["cb_id"]})).await.unwrap();
    assert_eq!(replay["result"]["body"], "adopted edit");
}

#[tokio::test]
async fn native_retry_preserves_audit_and_only_replaces_an_unedited_predecessor() {
    for (same_step, same_text, commit_old, explicit, accepted) in [
        (true, true, false, false, true),
        (false, true, false, false, false),
        (true, false, false, false, false),
        (true, true, true, false, false),
        (false, true, false, true, true),
    ] {
        let core = Core::new(Store::memory().await.unwrap());
        let mut auth = OperationContext { actor: "native-retry-owner".into(), allow_write: true, ..Default::default() };
        let chain = core.operate("chain.create", &auth, &json!({})).await.unwrap();
        auth.chain_id = chain["result"]["chain_id"].as_str().map(str::to_owned);
        let mut prior_request = json!({"format":"dsh-messages","request":{"messages":[
            {"role":"user","id":"old-wire-id","content":[{"type":"text","text":"original"}]}
        ]},"editable_unit_ids":["u1"]});
        if !explicit { prior_request["native_step_id"] = json!("a".repeat(64)); }
        let old = core.operate("native.begin", &auth, &prior_request).await.unwrap()["result"].clone();
        if commit_old {
            core.operate("native.commit", &auth, &json!({"source_id":old["source_id"],"expected_source_digest":old["source_digest"],
              "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"original","replacement":"older edit"}]})).await.unwrap();
        }
        let material = if same_text { "original" } else { "new input" };
        let mut next_request = json!({"format":"dsh-messages","request":{"messages":[
            {"role":"user","id":"new-wire-id","content":[{"type":"text","text":material}]}
        ]},"editable_unit_ids":["u1"]});
        if !explicit { next_request["native_step_id"] = json!(if same_step { "a".repeat(64) } else { "b".repeat(64) }); }
        let current = core.operate("native.begin", &auth, &next_request).await.unwrap()["result"].clone();
        core.operate("native.commit", &auth, &json!({"source_id":current["source_id"],"expected_source_digest":current["source_digest"],
          "expected_version":0,"edits":[{"unit_id":"u1","expected_text":material,"replacement":"new edit"}]})).await.unwrap();
        let mut publish = json!({"source_id":current["source_id"],"expected_source_digest":current["source_digest"],
          "expected_version":1,"expected_cut":0,"idempotency_key":"retry-publish"});
        if explicit {
            publish["replace_unpublished_source"] = json!({"source_id":old["source_id"],"expected_source_digest":old["source_digest"]});
        }
        let result = core.operate("native.publish", &auth, &publish).await;
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        let archived = core.operate("native.get", &auth, &json!({"source_id":old["source_id"]})).await.unwrap();
        assert_eq!(archived["result"]["source_request"], prior_request["request"]);
        assert!(archived["result"]["publication"].is_null());
    }
}
