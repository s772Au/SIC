use serde_json::{Value, json};
use sic_core_rust::{
    core::{Core, OperationContext},
    native_opaque,
    storage::{self, Store},
    transparent::NativeContext,
};
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
const WAV: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=";
fn fixtures() -> Vec<(&'static str, Value)> {
    let media = json!([{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{PNG}")}},
  {"type":"image_url","image_url":{"url":"https://media.invalid/image.png"}},
  {"type":"input_audio","input_audio":{"data":WAV,"format":"wav"}},
  {"type":"file","file":{"file_id":"file-original","filename":"original.pdf"}},
  {"type":"video_url","video_url":{"url":"https://media.invalid/original.mp4"}},
  {"type":"provider_future","text":"never editable","data":"AQIDBA==","signature":"signed-original"}]);
    vec![
        (
            "openai-chat",
            json!({"model":"original","stream":true,"n":3,"messages":[
  {"id":"assistant","role":"assistant","content":null,"audio":{"id":"native-audio","data":WAV,"transcript":"opaque transcript"},
   "tool_calls":[{"id":"call","type":"function","function":{"name":"inspect","arguments":"{\"text\":\"opaque\"}"}}]},
  {"id":"tool","role":"tool","tool_call_id":"call","content":"before"},{"id":"media","role":"user","content":media}]}),
        ),
        (
            "openai-responses",
            json!({"model":"original","stream":true,"previous_response_id":"native-prior","input":[
  {"type":"reasoning","id":"reason","encrypted_content":"signed-original"},
  {"type":"function_call","id":"function","call_id":"call","name":"inspect","arguments":"{\"text\":\"opaque\"}"},
  {"type":"function_call_output","id":"result","call_id":"call","output":[{"type":"input_text","text":"before"},
   {"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")},{"type":"input_file","file_id":"file-original"}]},
  {"role":"user","id":"media","content":media}]}),
        ),
        (
            "anthropic-messages",
            json!({"model":"original","max_tokens":123,"stream":true,"messages":[
  {"role":"assistant","id":"assistant","content":[{"type":"thinking","thinking":"opaque reasoning","signature":"signed-original"},
   {"type":"redacted_thinking","data":"opaque-original"},{"type":"tool_use","id":"call","name":"inspect","input":{"text":"opaque"}}]},
  {"role":"user","id":"tool","content":[{"type":"tool_result","tool_use_id":"call","content":[{"type":"text","text":"before"},
   {"type":"image","source":{"type":"base64","media_type":"image/png","data":PNG}},
   {"type":"document","source":{"type":"file","file_id":"file-original"}}]}]},
  {"role":"user","id":"media","content":media}]}),
        ),
        (
            "dsh-messages",
            json!({"provider_controls":{"model":"original","stream":true,"n":3},"messages":[
  {"role":"assistant","id":"assistant","content":[{"type":"reasoning","text":"opaque reasoning","signature":"signed-original"},
   {"type":"replay","provider":"original","payload":{"signed":"opaque"}},{"type":"tool-call","id":"call","name":"inspect","arguments":{"text":"opaque"}}]},
  {"role":"tool","id":"tool","toolCallId":"call","isError":false,"content":[{"type":"text","text":"before"},
   {"type":"image","source":{"type":"base64","mediaType":"image/png","data":PNG}},
   {"type":"attachment","attachment":{"id":"audio-original","url":"https://media.invalid/audio.wav","mediaType":"audio/wav"}},
   {"type":"attachment","attachment":{"id":"video-original","mediaType":"video/mp4"}}]},
  {"role":"user","id":"media","content":media}]}),
        ),
    ]
}
async fn op(core: &Core, a: &OperationContext, name: &str, p: Value) -> Value {
    core.operate(name, a, &p)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))["result"]
        .clone()
}
async fn setup(store: Store) -> (Core, OperationContext) {
    let core = Core::new(store);
    let mut a = OperationContext {
        actor: "media-owner".into(),
        allow_write: true,
        allow_revoke: true,
        ..Default::default()
    };
    a.chain_id = op(&core, &a, "chain.create", json!({})).await["chain_id"]
        .as_str()
        .map(str::to_owned);
    (core, a)
}
fn at<'a>(root: &'a Value, path: &Value) -> &'a Value {
    let mut at = root;
    for key in path.as_array().unwrap() {
        at = if let Some(key) = key.as_str() {
            &at[key]
        } else {
            &at[key.as_u64().unwrap() as usize]
        };
    }
    at
}
fn replace(root: &mut Value, path: &Value, text: &str) {
    let mut at = root;
    for key in path.as_array().unwrap() {
        at = if let Some(key) = key.as_str() {
            &mut at[key]
        } else {
            &mut at[key.as_u64().unwrap() as usize]
        };
    }
    *at = json!(text);
}
#[test]
fn native_carriers_extract_only_text_and_identify_immutable_blocks() {
    for (format, raw) in fixtures() {
        let c = NativeContext::new(format, &raw).unwrap();
        assert_eq!(c.units().len(), 1, "{format}");
        assert_eq!(c.units()[0].text, "before");
        assert_eq!(c.request().unwrap(), raw);
        let blocks = native_opaque::describe(format, &raw).unwrap();
        assert!(blocks.len() >= 6);
        for b in blocks {
            assert_eq!(
                b["canonical_sha256"],
                storage::digest(at(&raw, &b["native_anchor"])).unwrap()
            );
            assert_eq!(b["digest_encoding"], "native-block-canonical-json");
            assert_eq!(b["immutable"], true);
        }
    }
}
#[test]
fn responses_output_arrays_keep_existing_persisted_u_ids() {
    let raw = json!({"input":[{"type":"function_call_output","call_id":"old","output":[{"type":"input_text","text":"newly supported"}]},{"role":"user","content":"old text"}]});
    let c = NativeContext::new("openai-responses", &raw).unwrap();
    assert_eq!(c.units()[0].unit_id, "response-output:0:0");
    assert_eq!(c.units()[1].unit_id, "u1");
    assert_eq!(c.units()[1].text, "old text");
}

#[tokio::test]
async fn legacy_responses_archive_keeps_its_original_editable_scope() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let a = OperationContext {
        actor: "legacy-media-owner".into(),
        allow_write: true,
        ..Default::default()
    };
    let raw = json!({"input":[{"type":"function_call_output","call_id":"old","output":[
        {"type":"input_text","text":"legacy opaque text"},
        {"type":"input_image","image_url":"https://media.invalid/original.png"}
    ]},{"role":"user","content":"old text"}]});
    let format = "openai-responses";
    let digest = storage::digest(&json!({"format":format,"request":raw})).unwrap();
    let id = "legacy-responses-source";
    let archived = json!({"actor":a.actor,"chain_id":null,"core_source_id":null,
        "format":format,"request":raw,"source_digest":digest,"editable_unit_ids":["u1"],
        "protected_cb_ids":[],"protected_cb_bindings":[],"host_message_origins":[]});
    let mut tx = store.begin().await.unwrap();
    storage::put_object(&mut tx, "native_source", id, &archived)
        .await
        .unwrap();
    storage::put_object(
        &mut tx,
        "native_version",
        "legacy-responses-version",
        &json!({"source_id":id,"version":0,"request":raw,"receipt":null}),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let history = op(&core, &a, "native.history", json!({"source_id":id})).await;
    assert_eq!(history["opaque_blocks"].as_array().unwrap().len(), 1);
    assert_eq!(history["opaque_blocks"][0]["kind"], "input_image");
    let mut edit = json!({"source_id":id,"expected_source_digest":digest,
        "expected_version":0,"edits":[{"unit_id":"response-output:0:0","expected_text":"legacy opaque text","replacement":"unexpected"}]});
    assert_eq!(
        core.operate("native.commit", &a, &edit)
            .await
            .unwrap_err()
            .status,
        403
    );
    edit["edits"] = json!([{"unit_id":"u1","expected_text":"old text","replacement":"updated"}]);
    let result = op(&core, &a, "native.commit", edit).await;
    assert_eq!(result["request"]["input"][0], raw["input"][0]);
    assert_eq!(result["request"]["input"][1]["content"], "updated");
    let mut tx = store.begin().await.unwrap();
    assert_eq!(
        storage::get_object(&mut tx, id, Some("native_source"))
            .await
            .unwrap(),
        archived
    );
}

#[tokio::test]
async fn media_survives_pvi_recursive_text_delete_reopen_and_revocation() {
    for (format, raw) in fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.db");
        let (core, a) = setup(Store::create(&path).await.unwrap()).await;
        let s = op(
            &core,
            &a,
            "native.begin",
            json!({"format":format,"request":raw}),
        )
        .await;
        let unit = s["units"][0]["unit_id"].clone();
        let batch = json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,
   "ops":[{"op":"COMPRESS","target":unit,"content":"summary"}],"pvi":{"P":["persistent note"],"V":["next text"],"I":{"phase":"media unchanged"}}});
        let preview = op(&core, &a, "native.preview", batch.clone()).await;
        let mut commit = batch.clone();
        commit["edits"] = preview["edits"].clone();
        commit["delivery"] = json!({"ops":batch["ops"],"pvi":batch["pvi"]});
        op(&core, &a, "native.commit", commit).await;
        let done=op(&core,&a,"native.publish",json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":1,"expected_cut":0,"idempotency_key":"media-low"})).await;
        let mut expected = raw.clone();
        replace(
            &mut expected,
            &s["units"][0]["anchor"],
            "summary\npersistent note",
        );
        assert_eq!(done["request"], expected, "{format}");
        assert_eq!(done["source_request"], raw);
        assert_eq!(done["opaque_blocks"], s["opaque_blocks"]);
        let mut cb = done["publication"]["cb_id"].clone();
        for level in 2..=3 {
            let higher = op(
                &core,
                &a,
                "hierarchy.form",
                json!({"level":level,"cb_ids":[cb]}),
            )
            .await;
            let job = op(
                &core,
                &a,
                "edit.host.prepare",
                json!({"source_id":higher["source_id"]}),
            )
            .await;
            let frozen = job["source_snapshot"]["opaque_blocks"].as_array().unwrap();
            assert!(!frozen.is_empty());
            assert!(
                frozen
                    .iter()
                    .all(|b| b["source_id"] == s["binding"]["core_source_id"])
            );
            let claim = json!({"job_id":job["job_id"],"editor":"editor-1","claim_id":format!("media-{level}")});
            op(&core, &a, "edit.host.claim", claim.clone()).await;
            let mut submit = claim;
            submit["ops"] = json!([{"op":"DELETE","target":"u1"}]);
            submit["pvi"] = json!({"I":{"phase":format!("level-{level}")}});
            op(&core, &a, "edit.host.submit", submit).await;
            let published=op(&core,&a,"cut.publish",json!({"job_id":job["job_id"],"expected_cut":level-1,"idempotency_key":format!("media-{level}")})).await;
            cb = published["cb_id"].clone();
            assert_eq!(
                op(&core, &a, "memory.replay", json!({"cb_id":cb})).await["body"],
                ""
            );
        }
        let cut = op(&core, &a, "cut.get", json!({})).await;
        let bindings = cut["view"]["opaque_context_bindings"].as_array().unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["cb_id"], cb);
        assert_eq!(bindings[0]["editable_by_pvi"], false);
        assert_eq!(
            bindings[0]["blocks"].as_array().unwrap().len(),
            s["opaque_blocks"].as_array().unwrap().len()
        );
        let archived = op(
            &core,
            &a,
            "source.read",
            json!({"source_id":s["binding"]["core_source_id"]}),
        )
        .await;
        assert_eq!(archived["source"]["sidecar"], raw);
        core.store.close().await;
        drop(core);
        let core = Core::new(Store::open(&path).await.unwrap());
        assert_eq!(
            op(&core, &a, "native.get", json!({"source_id":s["source_id"]})).await["request"],
            expected
        );
        assert_eq!(
            op(&core, &a, "cut.get", json!({})).await["view"]["opaque_context_bindings"],
            cut["view"]["opaque_context_bindings"]
        );
        assert_eq!(
            op(
                &core,
                &a,
                "native.history",
                json!({"source_id":s["source_id"]})
            )
            .await["opaque_blocks"],
            s["opaque_blocks"]
        );
        let foreign = OperationContext {
            actor: "other".into(),
            ..a.clone()
        };
        assert_eq!(
            core.operate("native.get", &foreign, &json!({"source_id":s["source_id"]}))
                .await
                .unwrap_err()
                .status,
            403
        );
        op(
            &core,
            &a,
            "security.revoke",
            json!({"source_id":s["binding"]["core_source_id"]}),
        )
        .await;
        for name in ["cut.get", "native.get", "native.history"] {
            assert_eq!(
                core.operate(name, &a, &json!({"source_id":s["source_id"]}))
                    .await
                    .unwrap_err()
                    .status,
                403,
                "{format} {name}"
            );
        }
    }
}
#[tokio::test]
async fn media_only_is_passthrough_and_does_not_fabricate_text_or_a_cb() {
    let (core, a) = setup(Store::memory().await.unwrap()).await;
    for (format, mut raw) in fixtures() {
        let c = NativeContext::new(format, &raw).unwrap();
        let anchor = c.units()[0].anchor.as_array().unwrap();
        if format == "openai-chat" {
            raw["messages"][1]["content"] = Value::Null;
        } else {
            let parent = &anchor[..anchor.len() - 2];
            let mut at = &mut raw;
            for key in parent {
                at = if let Some(key) = key.as_str() {
                    &mut at[key]
                } else {
                    &mut at[key.as_u64().unwrap() as usize]
                };
            }
            at.as_array_mut().unwrap().remove(0);
        }
        let s = op(
            &core,
            &a,
            "native.begin",
            json!({"format":format,"request":raw,"editable_anchors":[]}),
        )
        .await;
        assert_eq!(s["request"], raw);
        assert!(s["units"].as_array().unwrap().is_empty());
        assert!(s["binding"]["core_source_id"].is_null());
        assert!(!s["opaque_blocks"].as_array().unwrap().is_empty());
        assert!(s["publication"].is_null());
    }
}
#[tokio::test]
async fn opaque_origin_proofs_never_grant_text_or_binary_editing_rights() {
    let (core, a) = setup(Store::memory().await.unwrap()).await;
    let raw = json!({"messages":[{"id":"native","role":"user","content":[{"type":"text","text":"before"},{"type":"image_url","image_url":{"url":"https://media.invalid/image.png"}}]}]});
    let text = json!(["messages", 0, "content", 0, "text"]);
    let image = json!(["messages", 0, "content", 1]);
    let p = json!({"format":"openai-chat","request":raw,"editable_anchors":[text],"host_message_origins":[{"message_id":"sdk","block_index":0,"native_anchor":text}],"host_opaque_origins":[{"message_id":"sdk","block_index":1,"native_anchor":image}]});
    for anchor in [
        text.clone(),
        json!(["messages", 0, "content", 1, "image_url", "url"]),
    ] {
        let mut bad = p.clone();
        bad["host_opaque_origins"][0]["native_anchor"] = anchor;
        assert_eq!(
            core.operate("native.begin", &a, &bad)
                .await
                .unwrap_err()
                .status,
            403
        );
    }
    let mut bad = p.clone();
    bad["host_opaque_origins"][0]["block_index"] = json!(0);
    assert_eq!(
        core.operate("native.begin", &a, &bad)
            .await
            .unwrap_err()
            .status,
        403
    );
    bad = p.clone();
    bad["editable_anchors"] = json!([["messages", 0, "content", 1, "image_url", "url"]]);
    assert_eq!(
        core.operate("native.begin", &a, &bad)
            .await
            .unwrap_err()
            .status,
        403
    );
    let s = op(&core, &a, "native.begin", p).await;
    assert_eq!(
        s["opaque_blocks"][0]["host_origin"],
        json!({"message_id":"sdk","block_index":1})
    );
    assert_eq!(s["binding"]["editable_unit_ids"], json!(["u1"]));
    assert_eq!(core.operate("native.commit",&a,&json!({"source_id":s["source_id"],"expected_source_digest":s["source_digest"],"expected_version":0,"edits":[{"unit_id":"opaque-image","expected_text":"https://media.invalid/image.png","replacement":"changed"}]})).await.unwrap_err().status,403);
}
