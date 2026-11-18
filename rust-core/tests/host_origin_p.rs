use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store};

async fn op(core:&Core,a:&OperationContext,name:&str,p:Value)->Value{
    core.operate(name,a,&p).await.unwrap()["result"].clone()
}

#[tokio::test]
async fn sdk_origin_coverage_does_not_claim_deleted_or_volatile_text_is_in_selected_p(){
    for route in ["KEEP","COMPRESS","DELETE","V"] {
        let core=Core::new(Store::memory().await.unwrap());
        let mut a=OperationContext{actor:"origin-owner".into(),allow_write:true,..Default::default()};
        a.chain_id=op(&core,&a,"chain.create",json!({})).await["chain_id"].as_str().map(str::to_owned);
        let source=op(&core,&a,"native.begin",json!({"format":"openai-responses","request":{"input":[{"role":"user","content":[
            {"type":"input_text","text":"actual tool result"},{"type":"input_text","text":"deleted tool secret"},{"type":"input_text","text":"user claim"}]}]},
            "host_message_origins":[
                {"message_id":"tool-retained","block_index":0,"native_anchor":["input",0,"content",0,"text"]},
                {"message_id":"tool-deleted","block_index":1,"native_anchor":["input",0,"content",1,"text"]},
                {"message_id":"user-claim","block_index":2,"native_anchor":["input",0,"content",2,"text"]}]})).await;
        op(&core,&a,"native.commit",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":0,
            "edits":[{"unit_id":"u2","expected_text":"deleted tool secret","replacement":""}],
            "delivery":{"ops":[{"op":"DELETE","target":"u2"}],"pvi":{}}})).await;
        let low=op(&core,&a,"native.publish",json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":1,
            "expected_cut":0,"idempotency_key":"origin-low","ops":[{"op":"DELETE","target":"u2"}],"pvi":{}})).await;
        let cut=op(&core,&a,"cut.get",json!({})).await;
        let origins=cut["view"]["cb_origin_bindings"][0]["origins"].as_array().unwrap();
        assert_eq!(origins.iter().map(|o|o["retained_in_p"].clone()).collect::<Vec<_>>(),vec![json!(true),json!(false),json!(true)]);
        assert_eq!(origins[0]["p_paths"],json!([[low["publication"]["cb_id"]]]));
        assert_eq!(origins[1]["p_paths"],json!([]));
        let formed=op(&core,&a,"hierarchy.form",json!({"level":2,"cb_ids":[low["publication"]["cb_id"]]})).await;
        let job=op(&core,&a,"edit.prepare",json!({"source_id":formed["source_id"]})).await;
        let target=job["editable_units"][0]["unit_id"].clone();
        let ops=match route {
            "KEEP"=>json!([]),
            "COMPRESS"=>json!([{"op":"COMPRESS","target":target,"content":"derived mixed-source representation"}]),
            "DELETE"=>json!([{"op":"DELETE","target":target}]),
            _=>json!([{"op":"COMPRESS","target":target,"content":"later representation","lifecycle":"V"}]),
        };
        op(&core,&a,"edit.submit",json!({"job_id":job["job_id"],"editor":"editor-1","ops":ops,"pvi":{}})).await;
        let high=op(&core,&a,"cut.publish",json!({"job_id":job["job_id"],"expected_cut":1,"idempotency_key":"origin-high"})).await;
        let cut=op(&core,&a,"cut.get",json!({})).await;
        let binding=&cut["view"]["cb_origin_bindings"][0];
        let origins=binding["origins"].as_array().unwrap();
        assert_eq!(origins.len(),3,"coverage must still suppress all archived original SDK units");
        assert_eq!(binding["missing_source_ids"],json!([]));
        let retained=["KEEP","COMPRESS"].contains(&route);
        for (n,origin) in origins.iter().enumerate(){
            assert_eq!(origin["retained_in_p"],retained&&n!=1,"{route}: {origin}");
            if retained&&n!=1 {assert_eq!(origin["p_paths"],json!([[high["cb_id"],low["publication"]["cb_id"]]]));}
            else {assert_eq!(origin["p_paths"],json!([]));}
        }
    }
}
