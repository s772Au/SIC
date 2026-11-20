use serde_json::{Value,json};
use sic_core_rust::{core::{Core,OperationContext},storage::Store};
#[tokio::test]
async fn post_publication_i_refresh_has_an_independent_retryable_receipt_and_never_mutates_cb_or_candidate() {
 let core=Core::new(Store::memory().await.unwrap());
 let mut a=OperationContext{actor:"I-owner".into(),allow_write:true,..Default::default()};
 a.chain_id=core.operate("chain.create",&a,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
 let cut=core.operate("cut.get",&a,&json!({})).await.unwrap()["result"].clone();assert!(cut["cut_id"].is_null());
 let section=format!("sic:instant-views\n{}",cut["instant_view_text"].as_str().unwrap());
 let raw=json!({"opaque":{"stream":true},"messages":[{"id":"material","role":"user","content":[{"type":"text","text":"before"}]},{"id":"runtime-I","role":"user","content":[{"type":"text","text":section}]}]});
 let begin=json!({"format":"dsh-messages","request":raw,"idempotency_key":"I-turn","editable_anchors":[["messages",0,"content",0,"text"]],
 "adopted_i":{"cut_id":cut["cut_id"],"view_digest":cut["instant_view_digest"],"native_anchor":["messages",1,"content",0,"text"],"section_text":section},
 "host_message_origins":[{"message_id":"runtime-I","block_index":0,"native_anchor":["messages",1,"content",0,"text"]}]});
 let s=core.operate("native.begin",&a,&begin).await.unwrap()["result"].clone();
 core.operate("native.commit",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"edits":[],"delivery":{"ops":[],"pvi":{"I":{"phase":"now"}}}})).await.unwrap();
 let chain=a.chain_id.clone().unwrap();
 core.operate("board.create",&a,&json!({"acl":{"readers":[chain],"writers":[chain],"claimers":[chain],"managers":[chain]}})).await.unwrap();
 let published=core.operate("native.publish",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"I-pub"})).await.unwrap()["result"].clone();
 assert_eq!(published["request"],raw);
 let cut=core.operate("cut.get",&a,&json!({})).await.unwrap()["result"].clone();
 let refresh=json!({"source_id":s["source_id"],"expected_version":1,"cut_id":cut["cut_id"],"idempotency_key":"I-current-worker"});
 let r=core.operate("native.refresh_i",&a,&refresh).await.unwrap()["result"].clone();
 let mut expected=raw.clone();expected["messages"][1]["content"][0]["text"]=json!(format!("sic:instant-views\n{}",cut["instant_view_text"].as_str().unwrap()));
 assert_eq!(r["request"],expected);assert_eq!(r["candidate_request"],raw);assert_eq!(r["version"],1);assert_eq!(r["source_request"],raw);
 assert_eq!(r["active_i_projection"]["host_origin"]["message_id"],"runtime-I");
 assert_eq!(r["active_i_projection"]["candidate_version_mutated"],false);
 let history=core.operate("native.history",&a,&json!({"source_id":s["source_id"],"limit":1})).await.unwrap();
 assert_eq!(history["result"]["i_projections"][0],r["active_i_projection"]);
 assert_eq!(history["result"]["has_more_i"],false);
 assert!(core.operate("native.history",&a,&json!({"source_id":s["source_id"],"after_i_revision":1})).await.unwrap()["result"]["i_projections"].as_array().unwrap().is_empty());
 let retry=core.operate("native.refresh_i",&a,&refresh).await.unwrap();assert_eq!(retry["evidence"]["replayed"],true);assert_eq!(retry["result"]["active_i_projection"],r["active_i_projection"]);
 assert_eq!(core.operate("native.begin",&a,&begin).await.unwrap()["result"]["request"],expected);
 let cb=core.operate("memory.replay",&a,&json!({"cb_id":published["publication"]["cb_id"]})).await.unwrap();assert_eq!(cb["result"]["body"],"before");
 let mut stale=refresh.clone();stale["cut_id"]=json!("stale");assert_eq!(core.operate("native.refresh_i",&a,&stale).await.unwrap_err().status,409);
 stale=refresh.clone();stale["expected_version"]=json!(0);assert_eq!(core.operate("native.refresh_i",&a,&stale).await.unwrap_err().status,409);
 core.operate("board.unbind",&a,&json!({})).await.unwrap();
 for op in ["native.history","native.get"] {
  assert_eq!(core.operate(op,&a,&json!({"source_id":s["source_id"]})).await.unwrap_err().status,403);
 }
}

#[tokio::test]
async fn archived_navigation_checks_indirect_board_dependencies_after_unbind() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut owner = OperationContext {actor:"archive-board-owner".into(),allow_write:true,..Default::default()};
    owner.chain_id = core.operate("chain.create",&owner,&json!({})).await.unwrap()["result"]["chain_id"].as_str().map(str::to_owned);
    let chain = owner.chain_id.as_deref().unwrap();
    core.operate("board.create",&owner,&json!({"acl":{"readers":[chain],"writers":[chain],"claimers":[chain],"managers":[chain]}})).await.unwrap();
    let cut = core.operate("cut.get",&owner,&json!({})).await.unwrap()["result"].clone();
    let section = format!("sic:instant-views\n{}",cut["instant_view_text"].as_str().unwrap());
    let first = json!({"format":"dsh-messages","request":{"messages":[
        {"id":"board-material","role":"user","content":[{"type":"text","text":"board-dependent material"}]},
        {"id":"frozen-i","role":"user","content":[{"type":"text","text":section}]}]},
        "editable_anchors":[["messages",0,"content",0,"text"]],
        "adopted_i":{"cut_id":cut["cut_id"],"view_digest":cut["instant_view_digest"],"native_anchor":["messages",1,"content",0,"text"],"section_text":section}});

    async fn publish(core: &Core, owner: &OperationContext, begin: Value, sequence:u64) -> (Value,Value) {
        let source = core.operate("native.begin",owner,&begin).await.unwrap()["result"].clone();
        core.operate("native.commit",owner,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
            "expected_version":0,"edits":[],"delivery":{"ops":[],"pvi":{}}})).await.unwrap();
        let result = core.operate("native.publish",owner,&json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],
            "expected_version":1,"expected_cut":sequence,"idempotency_key":format!("archive-board-{sequence}")})).await.unwrap();
        (source,result["result"]["publication"].clone())
    }
    let (_, lower) = publish(&core,&owner,first,0).await;
    let second = json!({"format":"dsh-messages","request":{"messages":[
        {"id":"fixed-cb","role":"user","content":[{"type":"text","text":"board-dependent material"}]},
        {"id":"derived-material","role":"user","content":[{"type":"text","text":"descendant source"}]}]},
        "protected_cb_bindings":[{"cb_id":lower["cb_id"],"native_anchors":[["messages",0,"content",0,"text"]]}],
        "editable_anchors":[["messages",1,"content",0,"text"]]});
    let (native, derived) = publish(&core,&owner,second,1).await;
    let page = json!({"source_id":native["source_id"],"unit_id":native["binding"]["editable_unit_ids"][0]});
    let read = core.operate("memory.source_text_page",&owner,&page).await.unwrap()["result"].clone();
    assert_eq!(read["text"],"descendant source");
    assert_eq!(read["original_message"]["role"],"user");
    assert_eq!(read["original_message"]["message_id"],"derived-material");
    let expand = json!({"cb_id":derived["cb_id"]});
    let ops = json!({"kind":"ops","object_id":derived["cb_id"]});
    core.operate("memory.archive_expand",&owner,&expand).await.unwrap();
    core.operate("memory.archive_ops",&owner,&ops).await.unwrap();
    core.operate("board.unbind",&owner,&json!({})).await.unwrap();
    for (tool,args) in [("memory.source_text_page",page),("memory.archive_expand",expand.clone()),
        ("memory.archive_ops",ops.clone()),("memory.page",ops),("hierarchy.expand",expand.clone()),
        ("memory.replay",expand),("memory.archive_search",json!({"query":"descendant"}))] {
        assert_eq!(core.operate(tool,&owner,&args).await.unwrap_err().status,403,"{tool}");
    }
}
