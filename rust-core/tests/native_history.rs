use serde_json::json;
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

#[tokio::test]
async fn history_pages_real_versions_and_respects_scope_and_revocation() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut auth = OperationContext {
        actor: "owner".into(),
        allow_write: true,
        allow_revoke: true,
        ..Default::default()
    };
    let chain = core
        .operate("chain.create", &auth, &json!({}))
        .await
        .unwrap();
    auth.chain_id = chain["result"]["chain_id"].as_str().map(str::to_owned);
    let begun = core
        .operate(
            "native.begin",
            &auth,
            &json!({"format":"openai-chat",
        "request":{"messages":[{"role":"user","content":"before"}],"n":2,"stream":true}}),
        )
        .await
        .unwrap();
    let source = &begun["result"];
    for (version, before, after) in [(0, "before", "middle"), (1, "middle", "after")] {
        core.operate(
            "native.commit",
            &auth,
            &json!({"source_id":source["source_id"],
            "expected_source_digest":source["source_digest"],"expected_version":version,
            "edits":[{"unit_id":"u1","expected_text":before,"replacement":after}]}),
        )
        .await
        .unwrap();
    }
    let query = json!({"source_id":source["source_id"],"limit":2});
    let first = core.operate("native.history", &auth, &query).await.unwrap();
    assert_eq!(
        first["result"]["source"]["request"]["messages"][0]["content"],
        "before"
    );
    assert_eq!(first["result"]["versions"].as_array().unwrap().len(), 2);
    assert_eq!(first["result"]["next_after_version"], 1);
    assert_eq!(first["result"]["has_more"], true);
    let last = core
        .operate(
            "native.history",
            &auth,
            &json!({"source_id":source["source_id"],"after_version":1}),
        )
        .await
        .unwrap();
    assert_eq!(
        last["result"]["versions"][0]["request"]["messages"][0]["content"],
        "after"
    );
    assert_eq!(last["result"]["has_more"], false);
    let published = core.operate("native.publish", &auth, &json!({"source_id":source["source_id"],
        "expected_source_digest":source["source_digest"],"expected_version":2,"expected_cut":0,"idempotency_key":"history-publication"})).await.unwrap();
    let trace = core.operate("native.history", &auth, &query).await.unwrap();
    assert_eq!(
        trace["result"]["publication"],
        published["result"]["publication"]
    );
    for caller in [
        OperationContext {
            actor: "other".into(),
            ..auth.clone()
        },
        OperationContext {
            chain_id: None,
            ..auth.clone()
        },
    ] {
        assert_eq!(
            core.operate("native.history", &caller, &query)
                .await
                .unwrap_err()
                .status,
            403
        );
    }
    for limit in [0, 101] {
        assert_eq!(
            core.operate(
                "native.history",
                &auth,
                &json!({"source_id":source["source_id"],"limit":limit})
            )
            .await
            .unwrap_err()
            .status,
            422
        );
    }
    core.operate(
        "security.revoke",
        &auth,
        &json!({"source_id":source["binding"]["core_source_id"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        core.operate("native.history", &auth, &query)
            .await
            .unwrap_err()
            .status,
        403
    );
}

#[tokio::test]
async fn historical_native_carriers_do_not_reveal_an_unbound_board_i_snapshot() {
    let core = Core::new(Store::memory().await.unwrap());
    let mut auth = OperationContext {
        actor: "board-history-owner".into(),
        allow_write: true,
        ..Default::default()
    };
    auth.chain_id = core
        .operate("chain.create", &auth, &json!({}))
        .await
        .unwrap()["result"]["chain_id"]
        .as_str()
        .map(str::to_owned);
    let id = auth.chain_id.clone().unwrap();
    core.operate(
        "board.create",
        &auth,
        &json!({"acl":{"readers":[id],"writers":[id],"claimers":[id],"managers":[id]}}),
    )
    .await
    .unwrap();
    core.operate("board.task.create",&auth,&json!({"expected_revision":1,"idempotency_key":"fixture-task","title":"fixture-board-title","detail":{}})).await.unwrap();
    let source = core
        .operate(
            "source.ingest",
            &auth,
            &json!({"request":{"messages":[{"role":"user","content":"body"}]}}),
        )
        .await
        .unwrap();
    let job = core
        .operate(
            "edit.program",
            &auth,
            &json!({"source_id":source["result"]["source_id"],"rules":[]}),
        )
        .await
        .unwrap();
    let published=core.operate("cut.publish",&auth,&json!({"job_id":job["result"]["job_id"],"expected_cut":0,"idempotency_key":"board-cut"})).await.unwrap();
    let cut = core.operate("cut.get", &auth, &json!({})).await.unwrap()["result"].clone();
    let section = format!(
        "sic:instant-views\n{}",
        cut["instant_view_text"].as_str().unwrap()
    );
    let carrier=core.operate("native.begin",&auth,&json!({"format":"openai-chat",
        "request":{"messages":[{"role":"user","content":"body"},{"role":"user","content":section}]},
        "editable_unit_ids":[],"protected_cb_bindings":[{"cb_id":published["result"]["cb_id"],"unit_ids":["u1"]}],
        "adopted_i":{"cut_id":cut["cut_id"],"view_digest":cut["instant_view_digest"],"native_anchor":["messages",1,"content"],"section_text":section}})).await.unwrap();
    let query = json!({"source_id":carrier["result"]["source_id"]});
    assert!(
        core.operate("native.history", &auth, &query)
            .await
            .unwrap()
            .to_string()
            .contains("fixture-board-title")
    );
    core.operate("board.unbind", &auth, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        core.operate("native.history", &auth, &query)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        core.operate("native.get", &auth, &query)
            .await
            .unwrap_err()
            .status,
        403
    );
}
