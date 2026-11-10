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
async fn op(core: &Core, c: &OperationContext, name: &str, p: Value) -> Value {
    core.operate(name, c, &p).await.unwrap()["result"].clone()
}
async fn chain(core: &Core, actor: &str) -> String {
    op(core, &ctx(actor, None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .into()
}
async fn publish(core: &Core, c: &OperationContext, text: &str, key: &str) {
    let source = op(
        core,
        c,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":text}]}}),
    )
    .await;
    let job = op(
        core,
        c,
        "edit.program",
        json!({"source_id":source["source_id"],"rules":[]}),
    )
    .await;
    let state = op(core, c, "chain.get", json!({})).await;
    op(
        core,
        c,
        "cut.publish",
        json!({"job_id":job["job_id"],"expected_cut":state["cut_seq"],"idempotency_key":key}),
    )
    .await;
}

#[tokio::test]
async fn shared_board_refreshes_i_on_publication_and_revocation_blocks_stale_views() {
    let core = Core::new(Store::memory().await.unwrap());
    let a = chain(&core, "alice").await;
    let b = chain(&core, "bob").await;
    let alice = ctx("alice", Some(&a));
    let bob = ctx("bob", Some(&b));
    let board = op(
        &core,
        &bob,
        "board.create",
        json!({"acl":{"readers":[a,b],"writers":[b],"claimers":[a],"managers":[b]}}),
    )
    .await;
    let id = board["board_id"].as_str().unwrap();
    op(&core, &alice, "board.bind", json!({"board_id":id})).await;
    let task = op(
        &core,
        &bob,
        "board.task.create",
        json!({"expected_revision":1,"idempotency_key":"task","title":"work","detail":{}}),
    )
    .await;
    let tid = task["task"]["task_id"].as_str().unwrap();
    op(
        &core,
        &alice,
        "board.task.claim",
        json!({"task_id":tid,"expected_revision":2,"idempotency_key":"claim"}),
    )
    .await;
    publish(&core, &alice, "CB one", "first").await;
    let first = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(first["view"]["instant"]["board"]["revision"], 3);
    assert_eq!(
        first["view"]["instant"]["board"]["tasks"][0]["status"],
        "claimed"
    );
    assert_eq!(first["view"]["cbs"][0]["body"], "CB one");
    assert!(core.operate("board.task.progress",&alice,&json!({"task_id":tid,"expected_revision":3,"idempotency_key":"forged-done","status":"done"})).await.is_err());
    op(&core,&alice,"board.task.progress",json!({"task_id":tid,"expected_revision":3,"idempotency_key":"progress","status":"in_progress"})).await;
    let before = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(before["view"]["instant"]["board"]["revision"], 3);
    publish(&core, &alice, "CB two", "second").await;
    let second = op(&core, &alice, "cut.get", json!({})).await;
    assert_eq!(second["view"]["instant"]["board"]["revision"], 4);
    assert_eq!(
        second["view"]["instant"]["board"]["tasks"][0]["status"],
        "in_progress"
    );
    assert_eq!(second["view"]["cbs"][1]["body"], "CB two");
    assert!(!second["view"]["cbs"].to_string().contains("in_progress"));
    op(
        &core,
        &bob,
        "board.acl.update",
        json!({"expected_revision":4,"idempotency_key":"revoke","acl":{
        "readers":[b],"writers":[b],"claimers":[],"managers":[b]}}),
    )
    .await;
    assert_eq!(
        core.operate("cut.get", &alice, &json!({}))
            .await
            .unwrap_err()
            .code,
        "forbidden"
    );
    op(&core, &alice, "board.unbind", json!({})).await;
    let clean = op(&core, &alice, "cut.get", json!({})).await;
    assert!(clean["view"]["instant"].get("board").is_none());
    assert!(!clean.to_string().contains("in_progress"));
    assert_eq!(clean["cut"]["projection"], "metadata-only");
}
