use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

fn ctx(actor: &str, chain: Option<&str>) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        chain_id: chain.map(str::to_owned),
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}

async fn op(core: &Core, context: &OperationContext, name: &str, params: Value) -> Value {
    core.operate(name, context, &params).await.unwrap()["result"].clone()
}

async fn new_chain(core: &Core, actor: &str) -> String {
    op(core, &ctx(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn publish(core: &Core, context: &OperationContext, source: &Value, key: &str) -> Value {
    let prepared = op(
        core,
        context,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await;
    let state = op(core, context, "chain.get", json!({})).await;
    op(
        core,
        context,
        "cut.publish",
        json!({
            "job_id":prepared["job_id"], "expected_cut":state["cut_seq"], "idempotency_key":key
        }),
    )
    .await
}

#[tokio::test]
async fn search_preserves_cb_wb_provenance_paging_and_late_source_membership() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = new_chain(&core, "alice").await;
    let alice = ctx("alice", Some(&chain));
    let input = op(&core, &alice, "source.ingest", json!({"request":{"messages":[
        {"role":"assistant","content":"前🙂needle 入口","tool_calls":[{"id":"call-1","type":"function","function":{"name":"read","arguments":"{}"}}]}
    ]}})).await;
    let job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":input["source_id"],"rules":[]}),
    )
    .await;
    let late = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[
            {"role":"tool","tool_call_id":"call-1","content":"迟到 needle 输出"}
        ]}}),
    )
    .await;
    let cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"p"}),
    )
    .await;
    let page1 = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"needle","limit":1}),
    )
    .await;
    assert_eq!(page1["hits"][0]["cb_id"], cut["cb_id"]);
    assert_eq!(page1["hits"][0]["wb_id"], cut["wb_id"]);
    assert_eq!(page1["hits"][0]["source_id"], input["source_id"]);
    assert_eq!(page1["hits"][0]["wb_source_membership"], "frozen_wb");
    assert_eq!(page1["hits"][0]["materialized_unit_id"], "u1");
    assert_eq!(page1["hits"][0]["match_offset"], 2);
    assert_eq!(
        page1["hits"][0]["source_anchor"]["native_path"],
        json!(["messages", 0, "content"])
    );
    assert_eq!(page1["complete"], false);
    let page2 = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"needle","limit":1,"cursor":page1["next_cursor"]}),
    )
    .await;
    assert_eq!(page2["hits"][0]["source_id"], late["part_source_id"]);
    assert_eq!(
        page2["hits"][0]["wb_source_membership"],
        "added_after_wb_formation"
    );
    assert_eq!(
        page2["hits"][0]["cb_source_membership"],
        "published_cb_source_snapshot"
    );
    let combined_id = format!("{}:u1", late["part_source_id"].as_str().unwrap());
    assert_eq!(page2["hits"][0]["materialized_unit_id"], combined_id);
    let published = op(&core, &alice, "cut.get", json!({})).await;
    let index = published["view"]["cbs"][0]["index"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["unit_id"] == combined_id)
        .unwrap();
    assert_eq!(
        page2["hits"][0]["operation_indexes"],
        index["operation_indexes"]
    );
    assert!(page2["hits"][0]["operation_index_count"].as_u64().unwrap() > 0);
    assert_eq!(page2["complete"], true);
    assert!(page2["next_cursor"].is_null());
    let full = op(
        &core,
        &alice,
        "memory.page",
        json!({"kind":"source","object_id":late["part_source_id"]}),
    )
    .await;
    assert!(full["value"].as_str().unwrap().contains("迟到 needle 输出"));

    let pending = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"needle pending"}]}}),
    )
    .await;
    let same_page = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"needle","limit":1,"cursor":page1["next_cursor"]}),
    )
    .await;
    assert_eq!(same_page["hits"], page2["hits"]);
    publish(&core, &alice, &pending["source_id"], "p2").await;
    assert_eq!(
        core.operate(
            "memory.search",
            &alice,
            &json!({"query":"needle","cursor":page1["next_cursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "search_cursor_stale"
    );
    assert_eq!(
        core.operate(
            "memory.search",
            &alice,
            &json!({"query":"other","cursor":page1["next_cursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "search_cursor_stale"
    );
    assert_eq!(
        core.operate(
            "memory.search",
            &ctx("bob", Some(&chain)),
            &json!({"query":"needle"})
        )
        .await
        .unwrap_err()
        .code,
        "forbidden"
    );
}

#[tokio::test]
async fn search_is_bounded_validates_cursors_and_excludes_other_contexts() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = new_chain(&core, "alice").await;
    let alice = ctx("alice", Some(&chain));
    let messages: Vec<Value> = (0..70)
        .map(|i| json!({"role":"user","content":if i == 69 {"end needle"} else {"no match"}}))
        .collect();
    let source = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":messages}}),
    )
    .await;
    publish(&core, &alice, &source["source_id"], "main").await;
    op(
        &core,
        &alice,
        "hierarchy.context.create",
        json!({"context_id":"private"}),
    )
    .await;
    let other = op(&core, &alice, "hierarchy.context.ingest", json!({"context_id":"private","request":{"messages":[{"role":"user","content":"private needle"}]}})).await;
    publish(&core, &alice, &other["source_id"], "private").await;
    let first = op(&core, &alice, "memory.search", json!({"query":"needle"})).await;
    assert_eq!(first["hits"], json!([]));
    assert_eq!(first["scanned_units"], 64);
    assert_eq!(first["complete"], false);
    let second = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"needle","cursor":first["next_cursor"]}),
    )
    .await;
    assert_eq!(second["hits"].as_array().unwrap().len(), 1);
    assert_eq!(second["hits"][0]["excerpt"], "end needle");
    assert_eq!(second["complete"], true);
    for bad in [
        json!({"query":""}),
        json!({"query":"x","limit":0}),
        json!({"query":"x","limit":9}),
        json!({"query":"x".repeat(97)}),
        json!({"query":"x","unknown":true}),
        json!({"query":"needle","cursor":{"snapshot":first["next_cursor"]["snapshot"],"cb_index":999,"source_index":0,"unit_index":0}}),
        json!({"query":"needle","cursor":{"snapshot":first["next_cursor"]["snapshot"],"cb_index":0,"source_index":999,"unit_index":0}}),
        json!({"query":"needle","cursor":{"snapshot":first["next_cursor"]["snapshot"],"cb_index":0,"source_index":0,"unit_index":999}}),
    ] {
        assert_eq!(
            core.operate("memory.search", &alice, &bad)
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
    }
    op(
        &core,
        &alice,
        "hierarchy.select",
        json!({"context_id":"private","level":1,"roles":["worker"]}),
    )
    .await;
    assert_eq!(
        core.operate(
            "memory.search",
            &alice,
            &json!({"query":"needle","cursor":first["next_cursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "search_cursor_stale"
    );
    let selected = op(&core, &alice, "memory.search", json!({"query":"needle"})).await;
    assert_eq!(selected["hits"].as_array().unwrap().len(), 1);
    assert_eq!(selected["hits"][0]["excerpt"], "private needle");
    let mut revoker = alice.clone();
    revoker.allow_revoke = true;
    op(
        &core,
        &revoker,
        "security.revoke",
        json!({"source_id":other["source_id"]}),
    )
    .await;
    assert_eq!(
        core.operate("memory.search", &alice, &json!({"query":"needle"}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
}

#[tokio::test]
async fn search_distinguishes_cb_only_content_from_sources_across_levels() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = new_chain(&core, "alice").await;
    let alice = ctx("alice", Some(&chain));
    let first = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"first source fact"}]}}),
    )
    .await;
    let job = op(
        &core,
        &alice,
        "edit.program",
        json!({"source_id":first["source_id"],"rules":[],"pvi":{"P":["持久-only-marker"]}}),
    )
    .await;
    let first_cut = op(
        &core,
        &alice,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":0,"idempotency_key":"first"}),
    )
    .await;
    let second = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"second source fact"}]}}),
    )
    .await;
    let second_cut = publish(&core, &alice, &second["source_id"], "second").await;
    let original = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"持久-only-marker","scope":"source"}),
    )
    .await;
    assert_eq!(original["hits"], json!([]));
    let body = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"持久-only-marker","scope":"cb"}),
    )
    .await;
    assert_eq!(body["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"][0]["match_kind"], "cb_body");
    assert_eq!(body["hits"][0]["cb_id"], first_cut["cb_id"]);
    assert_eq!(body["hits"][0]["wb_id"], first_cut["wb_id"]);
    assert!(body["hits"][0].get("source_id").is_none());
    let source_page = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"fact","scope":"source","limit":1}),
    )
    .await;
    assert!(!source_page["next_cursor"].is_null());
    assert_eq!(
        core.operate(
            "memory.search",
            &alice,
            &json!({"query":"fact","scope":"cb","cursor":source_page["next_cursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "search_cursor_stale"
    );
    let higher = op(
        &core,
        &alice,
        "hierarchy.form",
        json!({"level":2,"cb_ids":[first_cut["cb_id"],second_cut["cb_id"]]}),
    )
    .await;
    let higher_cut = publish(&core, &alice, &higher["source_id"], "higher").await;
    let high_source = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"持久-only-marker","scope":"source","level":2}),
    )
    .await;
    assert_eq!(high_source["hits"][0]["match_kind"], "source_unit");
    assert_eq!(high_source["hits"][0]["cb_id"], higher_cut["cb_id"]);
    assert_eq!(
        high_source["hits"][0]["source_anchor"]["cb_id"],
        first_cut["cb_id"]
    );
    let high_body = op(
        &core,
        &alice,
        "memory.search",
        json!({"query":"持久-only-marker","scope":"cb","level":2}),
    )
    .await;
    assert_eq!(high_body["hits"][0]["cb_id"], higher_cut["cb_id"]);
    for bad in [
        json!({"query":"x","scope":"all"}),
        json!({"query":"x","level":0}),
        json!({"query":"x","level":129}),
    ] {
        assert_eq!(
            core.operate("memory.search", &alice, &bad)
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
    }
}

#[tokio::test]
async fn pending_search_reports_unformed_and_formed_wb_without_inventing_cb() {
    let core = Core::new(Store::memory().await.unwrap());
    let chain = new_chain(&core, "alice").await;
    let alice = ctx("alice", Some(&chain));
    let first = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"pending needle one"}]}}),
    )
    .await;
    let second = op(
        &core,
        &alice,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"pending needle two"}]}}),
    )
    .await;
    let first_page = op(
        &core,
        &alice,
        "memory.search",
        json!({"scope":"pending","query":"needle","limit":1}),
    )
    .await;
    assert_eq!(first_page["hits"][0]["source_id"], first["source_id"]);
    assert_eq!(first_page["hits"][0]["match_kind"], "pending_source_unit");
    assert_eq!(first_page["hits"][0]["wb_source_membership"], "unformed");
    assert!(first_page["hits"][0]["wb_id"].is_null());
    assert!(first_page["hits"][0]["cb_id"].is_null());
    assert_eq!(first_page["next_cursor"]["root_index"], 1);
    let second_page = op(
        &core,
        &alice,
        "memory.search",
        json!({"scope":"pending","query":"needle","limit":1,"cursor":first_page["next_cursor"]}),
    )
    .await;
    assert_eq!(second_page["hits"][0]["source_id"], second["source_id"]);
    assert_eq!(second_page["complete"], true);
    assert_eq!(
        op(
            &core,
            &alice,
            "memory.search",
            json!({"scope":"source","query":"needle"})
        )
        .await["hits"],
        json!([])
    );
    let formed = op(
        &core,
        &alice,
        "wb.form",
        json!({"source_ids":[first["source_id"],second["source_id"]]}),
    )
    .await;
    assert_eq!(
        core.operate(
            "memory.search",
            &alice,
            &json!({"scope":"pending","query":"needle","cursor":first_page["next_cursor"]})
        )
        .await
        .unwrap_err()
        .code,
        "search_cursor_stale"
    );
    let pending = op(
        &core,
        &alice,
        "memory.search",
        json!({"scope":"pending","query":"needle","limit":2}),
    )
    .await;
    assert_eq!(pending["hits"].as_array().unwrap().len(), 2);
    for hit in pending["hits"].as_array().unwrap() {
        assert_eq!(hit["wb_id"], formed["wb_id"]);
        assert_eq!(hit["wb_source_membership"], "frozen_wb");
        assert!(hit["cb_id"].is_null());
    }
    let cb = publish(&core, &alice, &formed["source_id"], "finish").await;
    assert_eq!(
        op(
            &core,
            &alice,
            "memory.search",
            json!({"scope":"pending","query":"needle"})
        )
        .await["hits"],
        json!([])
    );
    let published = op(
        &core,
        &alice,
        "memory.search",
        json!({"scope":"source","query":"needle"}),
    )
    .await;
    assert_eq!(published["hits"][0]["cb_id"], cb["cb_id"]);
    assert_eq!(published["hits"][0]["wb_id"], formed["wb_id"]);
}
