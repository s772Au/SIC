use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    storage::Store,
};

#[tokio::test]
async fn native_versions_survive_database_reopen_and_independent_store_contention() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("owned-native-fixture.db");
    let owner = auth("fixture-owner", true);
    let store = Store::create(&path).await.unwrap();
    let core = Core::new(store.clone());
    let raw =
        json!({"model":"original","stream":true,"input":"before","previous_response_id":"opaque"});
    let source = core
        .operate(
            "native.begin",
            &owner,
            &json!({"format":"openai-responses","request":raw}),
        )
        .await
        .unwrap()["result"]
        .clone();
    core.operate(
        "native.commit",
        &owner,
        &commit(&source, 0, "before", "after"),
    )
    .await
    .unwrap();
    store.close().await;
    drop(core);
    drop(store);
    let first = Store::open(&path).await.unwrap();
    let second = Store::open(&path).await.unwrap();
    let a = Core::new(first.clone());
    let b = Core::new(second.clone());
    let current = a
        .operate(
            "native.get",
            &owner,
            &json!({"source_id":source["source_id"]}),
        )
        .await
        .unwrap();
    assert_eq!(current["result"]["version"], 1);
    assert_eq!(current["result"]["request"]["input"], "after");
    assert_eq!(
        current["result"]["request"]["previous_response_id"],
        "opaque"
    );
    let left = commit(&source, 1, "after", "left");
    let right = commit(&source, 1, "after", "right");
    let (left, right) = tokio::join!(
        a.operate("native.commit", &owner, &left),
        b.operate("native.commit", &owner, &right)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    first.close().await;
    second.close().await;
}

fn auth(actor: &str, write: bool) -> OperationContext {
    OperationContext {
        actor: actor.into(),
        allow_write: write,
        allow_model: false,
        ..Default::default()
    }
}
fn commit(source: &Value, base: u64, old: &str, new: &str) -> Value {
    json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"],"expected_version":base,
        "edits":[{"unit_id":"u1","expected_text":old,"replacement":new}]})
}

#[tokio::test]
async fn core_native_transactions_are_actor_scoped_atomic_and_persisted_without_model_profile() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let owner = auth("host-owner", true);
    let raw = json!({"model":"harness-model","stream":true,"n":2,"stream_options":{"include_usage":true},
        "vendor":{"opaque":[1,2]},"messages":[{"role":"system","content":"protected"},{"role":"user","content":"before"}]});
    assert!(
        core.operate(
            "native.begin",
            &auth("host-owner", false),
            &json!({"format":"openai-chat","request":raw})
        )
        .await
        .is_err()
    );
    let begun = core
        .operate(
            "native.begin",
            &owner,
            &json!({"format":"openai-chat","request":raw}),
        )
        .await
        .unwrap();
    assert_eq!(begun["evidence"]["provider_called"], false);
    let source = &begun["result"];
    assert_eq!(source["version"], 0);
    assert_eq!(source["units"].as_array().unwrap().len(), 1);
    let get = json!({"source_id":source["source_id"]});
    assert_eq!(
        core.operate("native.get", &auth("another", true), &get)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        core.operate(
            "native.commit",
            &auth("another", true),
            &commit(source, 0, "before", "wrong")
        )
        .await
        .unwrap_err()
        .status,
        403
    );
    let mut failed = commit(source, 0, "before", "partial");
    failed["edits"]
        .as_array_mut()
        .unwrap()
        .push(json!({"unit_id":"u999","expected_text":"protected","replacement":"forged"}));
    assert!(
        core.operate("native.commit", &owner, &failed)
            .await
            .is_err()
    );
    assert_eq!(
        core.operate("native.get", &owner, &get).await.unwrap()["result"]["request"],
        raw
    );
    let mut payload = commit(source, 0, "before", "after");
    payload["native_tool"] =
        json!({"name":"original_native_edit","arguments_sha256":"a".repeat(64)});
    let done = core
        .operate("native.commit", &owner, &payload)
        .await
        .unwrap();
    let mut expected = raw;
    expected["messages"][1]["content"] = json!("after");
    assert_eq!(done["result"]["request"], expected);
    assert_eq!(done["result"]["version"], 1);
    assert_eq!(
        done["result"]["receipt"]["native_tool"]["name"],
        "original_native_edit"
    );
    let retry = core.operate("native.commit", &owner, &payload).await.unwrap();
    assert_eq!(retry["evidence"]["replayed"],true);
    assert_eq!(retry["result"]["version"],1);
    let reopened = Core::new(store.clone());
    let current = reopened
        .operate("native.get", &auth("host-owner", false), &get)
        .await
        .unwrap();
    assert_eq!(current["result"], done["result"]);
    let left = commit(source, 1, "after", "left");
    let right = commit(source, 1, "after", "right");
    let (a, b) = tokio::join!(
        core.operate("native.commit", &owner, &left),
        reopened.operate("native.commit", &owner, &right)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        reopened.operate("native.get", &owner, &get).await.unwrap()["result"]["version"],
        2
    );
    store.close().await;
}
