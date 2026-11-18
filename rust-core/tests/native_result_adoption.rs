use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store,harness_lifecycle::{HarnessGrant,HarnessRole,HumanIntent}};
async fn grant(core:&Core,auth:&OperationContext,name:&str,id:Option<&str>,role:HarnessRole) -> String {
    core.issue_harness_grant(auth,HarnessGrant{scope_id:"session".into(),chain_id:auth.chain_id.clone(),turn_id:"trusted-turn".into(),
        human_message_id:None,role,human_intent:HumanIntent::None,allowed_calls:vec![name.into()],object_id:id.map(str::to_owned),
        goal_round:None,max_goal_rounds:None,wait_timeout_ms:None,launch:None}).await.unwrap()["grant_id"].as_str().unwrap().into()
}
async fn observe(core:&Core,auth:&OperationContext,id:&str,event:&str,value:Value) -> Value {
    let current = core.operate("harness.get",auth,&json!({"scope_id":"session","object_id":id})).await.unwrap()["result"].clone();
    let granted = grant(core,auth,value["kind"].as_str().unwrap(),Some(id),HarnessRole::Executor).await;
    core.operate("harness.observe",auth,&json!({"scope_id":"session","object_id":id,"expected_revision":current["revision"],
        "grant_id":granted,"event_id":event,"observation":value})).await.unwrap()["result"].clone()
}
#[tokio::test]
async fn only_verified_collected_job_text_can_claim_a_real_native_cb_adoption() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut auth = OperationContext{actor:"owner".into(),allow_write:true,..Default::default()};
    auth.chain_id = core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let granted = grant(&core,&auth,"job.register",None,HarnessRole::Worker).await;
    let registered = core.operate("harness.observe",&auth,&json!({"scope_id":"session","grant_id":granted,"event_id":"register",
        "observation":{"kind":"job.register","external_id":"real-child","label":"local child","started_at_ms":1,"output_mode":"stream",
            "executor":{"kind":"local-fixture","id":"child-process"}}})).await.unwrap()["result"].clone();
    let id = registered["object_id"].as_str().unwrap();
    observe(&core,&auth,id,"output",json!({"kind":"job.output","text":"x\nx"})).await;
    let granted = grant(&core,&auth,"job_output",Some(id),HarnessRole::Worker).await;
    let collected = core.operate("harness.call",&auth,&json!({"scope_id":"session","name":"job_output","arguments":{"job_id":"real-child"},
        "grant_id":granted,"event_id":"collect"})).await.unwrap()["result"].clone();
    let provenance = json!({"lifecycle_object_id":id,"collection_receipt_id":collected["receipt_id"],"unit_ids":["u1"]});
    for ids in [json!(["u1","u1"]),json!([]),json!(["u999"])] {
        let mut forged = provenance.clone();
        forged["unit_ids"] = ids;
        let error = core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"x"},
            "provenance":forged})).await.unwrap_err();
        assert_eq!(error.status,403,"duplicate, empty or unauthorized unit IDs must not synthesize collected output");
    }
    assert_eq!(core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"unrelated material"},
        "provenance":provenance})).await.unwrap_err().status,409);
    let source = core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"x\nx"},
        "provenance":provenance})).await.unwrap()["result"].clone();
    assert_eq!(source["binding"]["provenance"]["collection_digest"],collected["collection"]["text_digest"]);
    core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":0,"edits":[]})).await.unwrap();
    let published = core.operate("native.publish",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
        "expected_version":1,"expected_cut":0,"idempotency_key":"adopt-job-result"})).await.unwrap();
    let publication_id = &published["result"]["publication"]["publication_id"];
    observe(&core,&auth,id,"adopt",json!({"kind":"result.adopt","publication_id":publication_id,"collection_receipt_id":collected["receipt_id"]})).await;
    let state = core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();
    assert_eq!(state["result"]["adopted"],true);
    assert_eq!(state["result"]["adoptions"][0]["cb_id"],published["result"]["publication"]["cb_id"]);
}

async fn adoption_fixture() -> (Core,OperationContext,String,Value) {
    let core=Core::new(Store::memory().await.unwrap());
    let mut auth=OperationContext{actor:"adoption-owner".into(),allow_write:true,..Default::default()};
    auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let g=grant(&core,&auth,"job.register",None,HarnessRole::Worker).await;
    let registered=core.operate("harness.observe",&auth,&json!({"scope_id":"session","grant_id":g,"event_id":"register",
        "observation":{"kind":"job.register","external_id":"child","label":"fixture","started_at_ms":1,"output_mode":"stream",
            "executor":{"kind":"local-fixture","id":"child"}}})).await.unwrap()["result"].clone();
    let id=registered["object_id"].as_str().unwrap().to_owned();
    observe(&core,&auth,&id,"output",json!({"kind":"job.output","text":"raw result"})).await;
    let g=grant(&core,&auth,"job_output",Some(&id),HarnessRole::Worker).await;
    let collected=core.operate("harness.call",&auth,&json!({"scope_id":"session","name":"job_output","arguments":{"job_id":"child"},
        "grant_id":g,"event_id":"collect"})).await.unwrap()["result"].clone();
    (core,auth,id,collected)
}
async fn try_adopt(core:&Core,auth:&OperationContext,id:&str,event:&str,publication:&Value,collected:&Value) -> Result<Value,sic_core_rust::error::CoreError> {
    let state=core.operate("harness.get",auth,&json!({"scope_id":"session","object_id":id})).await.unwrap()["result"].clone();
    let g=grant(core,auth,"result.adopt",Some(id),HarnessRole::Executor).await;
    core.operate("harness.observe",auth,&json!({"scope_id":"session","object_id":id,"expected_revision":state["revision"],
        "grant_id":g,"event_id":event,"observation":{"kind":"result.adopt","publication_id":publication["publication_id"],
            "collection_receipt_id":collected["receipt_id"]}})).await
}
#[tokio::test]
async fn deletion_and_volatile_routing_cannot_claim_current_adoption_but_successor_v_cb_can() {
    for route in ["DELETE","V"] {
        let (core,auth,id,collected)=adoption_fixture().await;
        let proof=json!({"lifecycle_object_id":id,"collection_receipt_id":collected["receipt_id"],"unit_ids":["u1"]});
        let source=core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"raw result"},"provenance":proof})).await.unwrap()["result"].clone();
        core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
            "expected_version":0,"edits":[{"unit_id":"u1","expected_text":"raw result","replacement":""}]})).await.unwrap();
        let op=if route=="DELETE" {json!({"op":"DELETE","target":"u1"})}
            else {json!({"op":"COMPRESS","target":"u1","content":"result summary","lifecycle":"V"})};
        let done=core.operate("native.publish",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
            "expected_version":1,"expected_cut":0,"idempotency_key":"route-result","ops":[op],"pvi":{}})).await.unwrap();
        let first=&done["result"]["publication"];
        assert_eq!(first["publication"]["collection_adoption"]["eligible_current_p"],false);
        let probe=core.operate("harness.get",&OperationContext{allow_write:false,..auth.clone()},&json!({"scope_id":"session","object_id":id,"adoption_probe":{"publication_id":first["publication_id"],"collection_receipt_id":collected["receipt_id"]}})).await.unwrap();
        assert_eq!(probe["result"]["adoption_probe"]["eligible_current_p"],false);
        assert_eq!(try_adopt(&core,&auth,&id,"premature",first,&collected).await.unwrap_err().status,403);
        let state=core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();
        assert_eq!(state["result"]["adopted"],false);
        if route=="DELETE" {continue;}
        let v=&first["publication"]["next_working_material"]["source_id"];
        let second=core.operate("native.begin",&auth,&json!({"format":"openai-responses",
            "request":{"input":[{"role":"user","content":[{"type":"input_text","text":""}]},{"role":"user","content":[{"type":"input_text","text":"result summary"}]}]},
            "editable_unit_ids":["u2"],"protected_cb_bindings":[{"cb_id":first["cb_id"],"unit_ids":["u1"]}],
            "volatile_source_bindings":[{"source_id":v,"unit_ids":["u2"]}],
            "provenance":{"lifecycle_object_id":id,"collection_receipt_id":collected["receipt_id"],"unit_ids":["u2"],"via_volatile_source_id":v}
        })).await.unwrap()["result"].clone();
        core.operate("native.commit",&auth,&json!({"source_id":second["source_id"],"expected_source_digest":second["source_digest"],
            "expected_version":0,"edits":[]})).await.unwrap();
        let done=core.operate("native.publish",&auth,&json!({"source_id":second["source_id"],"expected_source_digest":second["source_digest"],
            "expected_version":1,"expected_cut":1,"idempotency_key":"successor","ops":[],"pvi":{}})).await.unwrap();
        let adopted=&done["result"]["publication"];
        assert_eq!(adopted["publication"]["collection_adoption"]["eligible_current_p"],true);
        try_adopt(&core,&auth,&id,"successor-adoption",adopted,&collected).await.unwrap();
        let state=core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();
        assert_eq!(state["result"]["adopted"],true);
        assert_eq!(state["result"]["adoptions"][0]["cb_id"],adopted["cb_id"]);
    }
}

#[tokio::test]
async fn multiple_job_collection_byte_ranges_follow_real_sdk_tool_leaves_and_adopt_independently() {
 let core=Core::new(Store::memory().await.unwrap());
 let mut auth=OperationContext{actor:"multi-collection".into(),allow_write:true,..Default::default()};
 auth.chain_id=core.operate("chain.create",&auth,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
 let output="甲\nx";
 let mut jobs=Vec::new();
 for index in 0..2 {
  let external=format!("child-{index}");
  let g=grant(&core,&auth,"job.register",None,HarnessRole::Worker).await;
  let r=core.operate("harness.observe",&auth,&json!({"scope_id":"session","grant_id":g,"event_id":format!("register-{index}"),
   "observation":{"kind":"job.register","external_id":external,"label":"fixture","started_at_ms":1,"output_mode":"stream","executor":{"kind":"local-fixture","id":external}}})).await.unwrap()["result"].clone();
  let id=r["object_id"].as_str().unwrap().to_owned();
  observe(&core,&auth,&id,&format!("output-{index}"),json!({"kind":"job.output","text":output})).await;
  let g=grant(&core,&auth,"job_output",Some(&id),HarnessRole::Worker).await;
  let c=core.operate("harness.call",&auth,&json!({"scope_id":"session","name":"job_output","arguments":{"job_id":external},"grant_id":g,"event_id":format!("collect-{index}")})).await.unwrap()["result"].clone();
  jobs.push((id,c));
 }
 let raw=json!({"messages":[{"role":"assistant","content":[{"type":"tool-call","id":"call-1","name":"job_output"},{"type":"tool-call","id":"call-2","name":"job_output"}]},
  {"id":"result-1","role":"tool","toolCallId":"call-1","content":[{"type":"text","text":format!("{output}\n[child-0 running]")}]},
  {"id":"result-2","role":"tool","toolCallId":"call-2","content":[{"type":"text","text":format!("{output}\n[child-1 running]")}]},
  {"id":"human","role":"user","content":[{"type":"text","text":"new human material"}]}]});
 let proofs:Vec<Value>=jobs.iter().enumerate().map(|(index,(id,c))|json!({"lifecycle_object_id":id,"collection_receipt_id":c["receipt_id"],"native_anchor":["messages",index+1,"content",0,"text"],"utf8_start":0,"utf8_end":output.len()})).collect();
 let begin=json!({"format":"dsh-messages","request":raw,"provenances":proofs});
 let mut duplicate=begin.clone();duplicate["provenances"]=json!([proofs[0],proofs[0]]);
 assert_eq!(core.operate("native.begin",&auth,&duplicate).await.unwrap_err().status,403);
 let mut overlap=begin.clone();overlap["provenances"][1]["native_anchor"]=proofs[0]["native_anchor"].clone();
 assert_eq!(core.operate("native.begin",&auth,&overlap).await.unwrap_err().status,403);
 let mut boundary=begin.clone();boundary["provenances"][0]["utf8_start"]=json!(1);
 assert_eq!(core.operate("native.begin",&auth,&boundary).await.unwrap_err().status,403);
 let mut wrong=begin.clone();wrong["provenances"][0]["utf8_end"]=json!(output.len()+1);
 assert_eq!(core.operate("native.begin",&auth,&wrong).await.unwrap_err().status,409);
 let mut empty=begin.clone();empty["provenances"][0]["utf8_end"]=json!(0);
 assert_eq!(core.operate("native.begin",&auth,&empty).await.unwrap_err().status,403);
 let mut readonly=begin.clone();readonly["editable_unit_ids"]=json!(["u1"]);
 assert_eq!(core.operate("native.begin",&auth,&readonly).await.unwrap_err().status,403);
 let s=core.operate("native.begin",&auth,&begin).await.unwrap()["result"].clone();
 assert!(s["binding"]["provenance"].is_null());assert_eq!(s["binding"]["provenances"].as_array().unwrap().len(),2);
 core.operate("native.commit",&auth,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"edits":[],"delivery":{"ops":[],"pvi":{}}})).await.unwrap();
 let done=core.operate("native.publish",&auth,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"multi-pub"})).await.unwrap()["result"].clone();
 assert_eq!(done["request"],raw);assert_eq!(done["publication"]["publication"]["collection_adoptions"].as_array().unwrap().len(),2);
 for (index,(id,c)) in jobs.iter().enumerate() {
  try_adopt(&core,&auth,id,&format!("adopt-{index}"),&done["publication"],c).await.unwrap();
  assert_eq!(core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap()["result"]["adopted"],true);
 }
}

#[tokio::test]
async fn final_selected_higher_p_lineage_controls_collection_adoption(){
 for route in ["KEEP","COMPRESS","REORDER","DELETE","V"] {
  let(core,auth,id,collected)=adoption_fixture().await;
  let low=core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"raw result"},"provenance":{"lifecycle_object_id":id,"collection_receipt_id":collected["receipt_id"],"unit_ids":["u1"]}})).await.unwrap()["result"].clone();
  core.operate("native.commit",&auth,&json!({"source_id":low["source_id"],"expected_source_digest":low["source_digest"],"expected_version":0,"edits":[]})).await.unwrap();
  let low=core.operate("native.publish",&auth,&json!({"source_id":low["source_id"],"expected_source_digest":low["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"lineage-low"})).await.unwrap()["result"]["publication"].clone();
  let higher=core.operate("hierarchy.form",&auth,&json!({"level":2,"cb_ids":[low["cb_id"]]})).await.unwrap()["result"].clone();
  let j=core.operate("edit.prepare",&auth,&json!({"source_id":higher["source_id"]})).await.unwrap()["result"].clone();
  let ops=match route {"KEEP"=>json!([]),"REORDER"=>json!([{"op":"REORDER","target":"u1","order":-1}]),"DELETE"=>json!([{"op":"DELETE","target":"u1"}]),"V"=>json!([{"op":"COMPRESS","target":"u1","content":"summary","lifecycle":"V"}]),_=>json!([{"op":"COMPRESS","target":"u1","content":"summary"}])};
  core.operate("edit.submit",&auth,&json!({"job_id":j["job_id"],"editor":"editor-1","ops":ops,"pvi":{}})).await.unwrap();
  let mut high=core.operate("cut.publish",&auth,&json!({"job_id":j["job_id"],"expected_cut":1,"idempotency_key":"lineage-high"})).await.unwrap()["result"].clone();
  let mut cut=core.operate("cut.get",&auth,&json!({})).await.unwrap();assert_eq!(cut["result"]["view"]["cbs"][0]["cb_id"],high["cb_id"]);
  // Simulate an archive produced before p_active existed; runtime must replay
  // its immutable engine records rather than changing that stored archive.
  if ["COMPRESS","V"].contains(&route) {
   let mut archived=cut["result"]["view"]["cbs"][0].clone();for row in archived["index"].as_array_mut().unwrap(){row.as_object_mut().unwrap().remove("p_active");}
   // Insert a pre-field archive fixture under its own immutable identity.
   // Existing CBs and cuts are never updated, even in this fixture.
   let legacy_id=format!("legacy-{}",high["cb_id"].as_str().unwrap());archived["cb_id"]=json!(legacy_id);
   let mut tx=core.store.begin().await.unwrap();sic_core_rust::storage::put_object(&mut tx,"cb",&legacy_id,&archived).await.unwrap();
   let mut state=sic_core_rust::storage::get_chain(&mut tx,auth.chain_id.as_deref().unwrap(),&auth.actor).await.unwrap();
   state["contexts"]["main"]["levels"]["2"]["cbs"]=json!([legacy_id]);state["contexts"]["main"]["levels"]["2"]["pairs"][archived["wb_id"].as_str().unwrap()]=json!(legacy_id);
   sic_core_rust::storage::save_chain(&mut tx,&state).await.unwrap();tx.commit().await.unwrap();
   core.operate("hierarchy.select",&auth,&json!({"level":1,"roles":["worker","editor"]})).await.unwrap();
   core.operate("hierarchy.select",&auth,&json!({"level":2,"roles":["worker","editor"]})).await.unwrap();high["cb_id"]=json!(legacy_id);
   cut=core.operate("cut.get",&auth,&json!({})).await.unwrap();
  }
  // Probe is an authorized read, not a grant or a result.adopt mutation.
  let readonly=sic_core_rust::core::OperationContext{allow_write:false,..auth.clone()};
  let plain=json!({"scope_id":"session","object_id":id});let before=core.operate("harness.get",&readonly,&plain).await.unwrap()["result"].clone();
  let mut tx=core.store.begin().await.unwrap();let object_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM objects").fetch_one(&mut *tx).await.unwrap();tx.commit().await.unwrap();
  let query=json!({"scope_id":"session","object_id":id,"adoption_probe":{"publication_id":low["publication_id"],"collection_receipt_id":collected["receipt_id"]}});
  let probed=core.operate("harness.get",&readonly,&query).await.unwrap()["result"].clone();
  assert_eq!(probed["adoption_probe"]["eligible_current_p"],!["DELETE","V"].contains(&route));assert_eq!(probed["adoption_probe"]["adopted_cut_id"],cut["result"]["cut_id"]);
  assert_eq!(probed["revision"],before["revision"]);assert_eq!(probed["adopted"],before["adopted"]);
  let after=core.operate("harness.get",&readonly,&plain).await.unwrap()["result"].clone();assert_eq!(after,before);
  let mut tx=core.store.begin().await.unwrap();let after_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM objects").fetch_one(&mut *tx).await.unwrap();assert_eq!(after_count,object_count);tx.commit().await.unwrap();
  let mut bad=query.clone();bad["adoption_probe"]["adopted_cut_id"]=json!("not-an-authorized-cut");assert_eq!(core.operate("harness.get",&readonly,&bad).await.unwrap_err().status,403);
  let result=try_adopt(&core,&auth,&id,"final-cut-adoption",&low,&collected).await;
  if ["DELETE","V"].contains(&route) {
   assert_eq!(result.unwrap_err().status,403,"{route} cannot be adopted from the selected higher P");
   let state=core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();assert_eq!(state["result"]["adopted"],false);
   let current=state["result"].clone();let g=grant(&core,&auth,"result.adopt",Some(&id),HarnessRole::Executor).await;
   let observation=json!({"kind":"result.adopt","publication_id":low["publication_id"],"collection_receipt_id":collected["receipt_id"],"adopted_cut_id":low["publication"]["cut_id"]});
   let mut unrelated=observation.clone();unrelated["adopted_cut_id"]=json!("unrelated-chain-cut");
   assert_eq!(core.operate("harness.observe",&auth,&json!({"scope_id":"session","object_id":id,"expected_revision":current["revision"],"grant_id":g,"event_id":"unrelated-adoption-cut","observation":unrelated})).await.unwrap_err().status,403);
   core.operate("harness.observe",&auth,&json!({"scope_id":"session","object_id":id,"expected_revision":current["revision"],"grant_id":g,"event_id":"historical-inflight-adoption","observation":observation})).await.unwrap();
   let state=core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();
   assert_eq!(state["result"]["adoptions"][0]["adopted_cut_id"],low["publication"]["cut_id"]);assert_eq!(state["result"]["adoptions"][0]["p_lineage"]["historical_snapshot"],true);
  } else {
   result.unwrap();let state=core.operate("harness.get",&auth,&json!({"scope_id":"session","object_id":id})).await.unwrap();
   assert_eq!(state["result"]["adoptions"][0]["adopted_cut_id"],cut["result"]["cut_id"]);
   assert_eq!(state["result"]["adoptions"][0]["p_lineage"]["p_paths"][0],json!([high["cb_id"],low["cb_id"]]));
  }
  if ["COMPRESS","V"].contains(&route) {let mut tx=core.store.begin().await.unwrap();let archived=sic_core_rust::storage::get_object(&mut tx,high["cb_id"].as_str().unwrap(),Some("cb")).await.unwrap();assert!(archived["index"][0].get("p_active").is_none(),"legacy archive must stay immutable");}
 }
}

#[tokio::test]
async fn actual_higher_v_restored_into_next_p_preserves_collection_and_tool_origin_lineage(){
 for legacy in [false,true] {
  let(core,auth,id,collected)=adoption_fixture().await;
  let source=core.operate("native.begin",&auth,&json!({"format":"openai-responses","request":{"input":"raw result"},"host_message_origins":[{"message_id":"actual-job-output","block_index":0,"native_anchor":["input"]}],"provenance":{"lifecycle_object_id":id,"collection_receipt_id":collected["receipt_id"],"unit_ids":["u1"]}})).await.unwrap()["result"].clone();
  core.operate("native.commit",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,"edits":[]})).await.unwrap();
  let low=core.operate("native.publish",&auth,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"v-lineage-low"})).await.unwrap()["result"]["publication"].clone();
  let h=core.operate("hierarchy.form",&auth,&json!({"level":2,"cb_ids":[low["cb_id"]]})).await.unwrap()["result"].clone();let j=core.operate("edit.prepare",&auth,&json!({"source_id":h["source_id"]})).await.unwrap()["result"].clone();
  core.operate("edit.submit",&auth,&json!({"job_id":j["job_id"],"editor":"editor-1","ops":[{"op":"COMPRESS","target":"u1","content":"derived-result","lifecycle":"V"}],"pvi":{"V":["unattributed-V"]}})).await.unwrap();
  let high=core.operate("cut.publish",&auth,&json!({"job_id":j["job_id"],"expected_cut":1,"idempotency_key":"v-lineage-high"})).await.unwrap()["result"].clone();
  let query=json!({"scope_id":"session","object_id":id,"adoption_probe":{"publication_id":low["publication_id"],"collection_receipt_id":collected["receipt_id"]}});
  assert_eq!(core.operate("harness.get",&auth,&query).await.unwrap()["result"]["adoption_probe"]["eligible_current_p"],false);
  if legacy {
   let mut tx=core.store.begin().await.unwrap();let mut raw=sic_core_rust::storage::get_object(&mut tx,high["volatile_source_id"].as_str().unwrap(),Some("source")).await.unwrap();raw.as_object_mut().unwrap().remove("volatile_derivations");for unit in raw["units"].as_array_mut().unwrap(){unit["anchor"].as_object_mut().unwrap().remove("volatile_unit_id");}
   sic_core_rust::storage::put_object(&mut tx,"source","legacy-higher-V-source",&raw).await.unwrap();let mut state=sic_core_rust::storage::get_chain(&mut tx,auth.chain_id.as_deref().unwrap(),&auth.actor).await.unwrap();state["groups"]["legacy-higher-V-source"]=json!(["legacy-higher-V-source"]);state["contexts"]["main"]["levels"]["2"]["pending"]=json!(["legacy-higher-V-source"]);sic_core_rust::storage::save_chain(&mut tx,&state).await.unwrap();tx.commit().await.unwrap();
  }
  let p=serde_json::to_value(sic_core_rust::scheduler::SchedulerPolicy{editor_mode:sic_core_rust::scheduler::EditorMode::Host,min_level:2,max_level:2,level_batch_sizes:[(2,2)].into(),flush:true,max_model_calls:0,..Default::default()}).unwrap();
  let planned=core.operate("runtime.schedule",&auth,&p).await.unwrap()["result"].clone();assert!(planned["failure"].is_null(),"{planned}");let j=&planned["host_jobs"][0];assert_eq!(j["units"].as_array().unwrap().len(),2);let claim=json!({"job_id":j["job_id"],"editor":"editor-1","claim_id":"actual-V-to-P"});core.operate("edit.host.claim",&auth,&claim).await.unwrap();core.operate("edit.host.submit",&auth,&claim).await.unwrap();
  let done=core.operate("runtime.schedule",&auth,&p).await.unwrap()["result"].clone();assert!(done["failure"].is_null(),"{done}");let cb=&done["published"][0]["cb_id"];
  let cut=core.operate("cut.get",&auth,&json!({})).await.unwrap()["result"].clone();let binding=cut["view"]["cb_origin_bindings"].as_array().unwrap().iter().find(|b|b["cb_id"]==*cb).unwrap();assert_eq!(binding["missing_source_ids"],json!([]));let origin=&binding["origins"][0];assert_eq!(origin["message_id"],"actual-job-output");assert_eq!(origin["retained_in_p"],true);assert_eq!(origin["p_spans"][0]["relation"],"derived_scope");let body=cut["view"]["cbs"].as_array().unwrap().iter().find(|b|b["cb_id"]==*cb).unwrap()["body"].as_str().unwrap();let range=&origin["p_text_ranges"][0];assert_eq!(&body[range["utf8_start"].as_u64().unwrap() as usize..range["utf8_end"].as_u64().unwrap() as usize],"derived-result");
  assert_eq!(core.operate("harness.get",&auth,&query).await.unwrap()["result"]["adoption_probe"]["eligible_current_p"],true);try_adopt(&core,&auth,&id,"actual-restored-V-adoption",&low,&collected).await.unwrap();
 }
}
