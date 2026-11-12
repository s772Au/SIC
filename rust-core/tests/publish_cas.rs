use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};
use tempfile::tempdir;

fn context(chain: Option<String>) -> OperationContext {
    OperationContext {
        actor: "owner".into(),
        chain_id: chain,
        allow_write: true,
        allow_model: false,
        allow_revoke: false,
    }
}
async fn call(core: &Core, ctx: &OperationContext, op: &str, payload: Value) -> Value {
    core.operate(op, ctx, &payload)
        .await
        .unwrap_or_else(|error| panic!("{op} failed: {error:?}"))["result"]
        .clone()
}
async fn main_job(core: &Core, ctx: &OperationContext) -> (String, String) {
    let source = call(
        core,
        ctx,
        "source.ingest",
        json!({"request":{"messages":[{"role":"user","content":"main body"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(core, ctx, "wb.form", json!({"source_ids":[source]})).await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source, job)
}
async fn private_job(core: &Core, ctx: &OperationContext) -> (String, String) {
    let source = call(
        core,
        ctx,
        "hierarchy.context.ingest",
        json!({"context_id":"editor:e1",
               "request":{"messages":[{"role":"user","content":"private body"}]}}),
    )
    .await["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        core,
        ctx,
        "wb.form",
        json!({"context_id":"editor:e1","source_ids":[source]}),
    )
    .await;
    let job = call(
        core,
        ctx,
        "edit.program",
        json!({"source_id":source,"rules":[]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source, job)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_publish_at_one_cut_admits_one_and_rejects_the_stale_job() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("publish-cas.sqlite");
    let store = Store::create_with_connections(&path, 4).await.unwrap();
    let core = Core::new(store.clone());
    let other_store = Store::open_with_connections(&path, 4).await.unwrap();
    let other_core = Core::new(other_store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    call(
        &core,
        &ctx,
        "hierarchy.context.create",
        json!({"context_id":"editor:e1","editor":"e1"}),
    )
    .await;
    let (main_source, main_job_id) = main_job(&core, &ctx).await;
    let (private_source, private_job_id) = private_job(&core, &ctx).await;
    let main_payload = json!({"job_id":main_job_id,"expected_cut":0,"idempotency_key":"cas-main"});
    let private_payload =
        json!({"job_id":private_job_id,"expected_cut":0,"idempotency_key":"cas-private"});
    let (main, private) = tokio::join!(
        core.operate("cut.publish", &ctx, &main_payload),
        other_core.operate("cut.publish", &ctx, &private_payload)
    );
    assert_ne!(
        main.is_ok(),
        private.is_ok(),
        "exactly one publish may win at cut 0"
    );
    let (winner_payload, winner, loser_payload, loser_job, loser_is_main) = if main.is_ok() {
        (
            main_payload,
            main.unwrap(),
            private_payload,
            private_job_id.clone(),
            false,
        )
    } else {
        (
            private_payload,
            private.unwrap(),
            main_payload,
            main_job_id.clone(),
            true,
        )
    };
    assert_eq!(winner["result"]["sequence"], 1);
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 1);
    assert_eq!(state["jobs"][&loser_job]["status"], "superseded");
    let loser = core
        .operate("cut.publish", &ctx, &loser_payload)
        .await
        .unwrap_err();
    assert_eq!(loser.code, "conflict");

    // Re-prepare the loser at the new cut and publish it.
    let retry_source = if loser_is_main {
        &main_source
    } else {
        &private_source
    };
    let retry_job = call(
        &core,
        &ctx,
        "edit.program",
        json!({"source_id":retry_source,"rules":[]}),
    )
    .await["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let retry_payload = json!({"job_id":retry_job,"expected_cut":1,"idempotency_key":"cas-retry"});
    let published = call(&core, &ctx, "cut.publish", retry_payload).await;
    assert_eq!(published["sequence"], 2);
    assert_eq!(
        call(&core, &ctx, "chain.get", json!({})).await["cut_seq"],
        2
    );

    // Replaying the winning key returns the same cut without another append.
    let replay = call(&core, &ctx, "cut.publish", winner_payload).await;
    assert_eq!(replay["sequence"], 1);
    assert_eq!(
        call(&core, &ctx, "chain.get", json!({})).await["cut_seq"],
        2
    );
    other_store.close().await;
    store.close().await;
}
#[tokio::test]
async fn publish_replay_after_a_lost_confirmation_does_not_append_twice() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let (_source, job) = main_job(&core, &ctx).await;
    let payload = json!({"job_id":job,"expected_cut":0,"idempotency_key":"lost-confirm"});
    let first = call(&core, &ctx, "cut.publish", payload.clone()).await;
    let first_cb = first["cb_id"].as_str().unwrap().to_owned();
    assert_eq!(first["sequence"], 1);
    let replay = call(&core, &ctx, "cut.publish", payload.clone()).await;
    assert_eq!(replay["sequence"], 1);
    assert_eq!(replay["cb_id"].as_str().unwrap(), first_cb);
    let conflicting = core
        .operate(
            "cut.publish",
            &ctx,
            &json!({"job_id":job,"expected_cut":1,"idempotency_key":"lost-confirm"}),
        )
        .await
        .unwrap_err();
    assert_eq!(conflicting.code, "conflict");
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 1);
    assert_eq!(
        state["contexts"]["main"]["levels"]["1"]["cbs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    store.close().await;
}
#[tokio::test]
async fn publish_receipt_survives_store_reopen_and_replays_once() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("publish-receipt.sqlite");
    let store = Store::create_with_connections(&path, 4).await.unwrap();
    let core = Core::new(store.clone());
    let chain = call(&core, &context(None), "chain.create", json!({})).await["chain_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = context(Some(chain));
    let (_source, job) = main_job(&core, &ctx).await;
    let payload = json!({"job_id":job,"expected_cut":0,"idempotency_key":"durable-receipt"});
    let first = call(&core, &ctx, "cut.publish", payload.clone()).await;
    assert_eq!(first["sequence"], 1);
    let cb = first["cb_id"].as_str().unwrap().to_owned();
    drop(core);
    store.close().await;

    let reopened = Store::open_with_connections(&path, 4).await.unwrap();
    let core = Core::new(reopened.clone());
    let replay = call(&core, &ctx, "cut.publish", payload.clone()).await;
    assert_eq!(replay["sequence"], 1);
    assert_eq!(replay["cb_id"].as_str().unwrap(), cb);
    let state = call(&core, &ctx, "chain.get", json!({})).await;
    assert_eq!(state["cut_seq"], 1);
    assert_eq!(
        state["contexts"]["main"]["levels"]["1"]["cbs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    reopened.close().await;

    let again = Store::open_with_connections(&path, 4).await.unwrap();
    let core = Core::new(again.clone());
    let second_replay = call(&core, &ctx, "cut.publish", payload).await;
    assert_eq!(second_replay["sequence"], 1);
    assert_eq!(second_replay["cb_id"].as_str().unwrap(), cb);
    assert_eq!(
        call(&core, &ctx, "chain.get", json!({})).await["cut_seq"],
        1
    );
    again.close().await;
}
