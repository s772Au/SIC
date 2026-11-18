#[path = "../src/protocol.rs"]
mod protocol;
#[path = "../src/transparent.rs"]
mod transparent;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::Response,
    routing::post,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use transparent::{NativeContext, TextEdit};

fn edit(unit_id: &str, old: &str, new: &str) -> TextEdit {
    TextEdit {
        unit_id: unit_id.into(),
        expected_text: old.into(),
        replacement: new.into(),
    }
}

#[test]
fn all_native_protocols_preserve_opaque_options_and_only_edit_known_text() {
    let fixtures = [
        (
            "openai-chat",
            json!({"model":"harness/owned", "stream":true,"n":2,
            "stream_options":{"include_usage":true},"provider_extension":{"anything":[1,2]},
            "tools":[{"type":"custom","format":{"type":"grammar"}}],
            "messages":[{"role":"developer","content":"protected"},
                {"role":"user","content":[{"type":"text","text":"before","annotation":7},
                    {"type":"vendor_media","payload":"opaque"}]}]}),
            "/messages/1/content/0/text",
        ),
        (
            "openai-responses",
            json!({"model":"harness/owned", "stream":true,
            "previous_response_id":"previous","reasoning":{"effort":"high"},
            "instructions":"protected","tools":[{"type":"web_search_preview"}],
            "input":[{"type":"reasoning","encrypted_content":"opaque"},
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"before","cache_control":{"type":"ephemeral"}},
                    {"type":"input_file","file_id":"opaque"}]}]}),
            "/input/1/content/0/text",
        ),
        (
            "anthropic-messages",
            json!({"model":"harness/owned", "stream":true,
            "thinking":{"type":"enabled","budget_tokens":1000},
            "system":[{"type":"text","text":"protected","cache_control":{"type":"ephemeral"}}],
            "messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"opaque","signature":"signed"}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"owned",
                    "is_error":false,"content":[{"type":"text","text":"before"},
                        {"type":"image","source":{"type":"url","url":"https://invalid.example/image"}}]}]}]}),
            "/messages/1/content/0/content/0/text",
        ),
    ];
    for (format, raw, pointer) in fixtures {
        assert!(
            protocol::decode(format, &raw).is_err(),
            "original strict boundary rejects expanded native controls"
        );
        let mut context = NativeContext::new(format, &raw).unwrap();
        assert_eq!(context.request().unwrap(), raw);
        assert_eq!(context.units().len(), 1);
        let source = context.source_digest().to_string();
        let receipt = context
            .commit(&source, 0, &[edit("u1", "before", "after")])
            .unwrap();
        let mut expected = raw.clone();
        *expected.pointer_mut(pointer).unwrap() = json!("after");
        assert_eq!(context.request().unwrap(), expected);
        assert_eq!(receipt["version"], 1);
        assert_eq!(receipt["changes"][0]["unit_id"], "u1");
        assert_ne!(
            receipt["changes"][0]["before_sha256"],
            receipt["changes"][0]["after_sha256"]
        );
        assert_eq!(context.source_digest(), source);
    }
    let raw = json!({"model":"owned","input":"before","background":true,"stream":true});
    let mut context = NativeContext::new("openai-responses", &raw).unwrap();
    let source = context.source_digest().to_string();
    context
        .commit(&source, 0, &[edit("u1", "before", "after")])
        .unwrap();
    assert_eq!(
        context.request().unwrap(),
        json!({"model":"owned","input":"after","background":true,"stream":true})
    );
}

#[test]
fn edits_are_atomic_versioned_and_source_bound() {
    let raw = json!({"messages":[{"role":"system","content":"protected"},
        {"role":"user","content":"one"},{"role":"assistant","content":"two"}]});
    let mut context = NativeContext::new("openai-chat", &raw).unwrap();
    let source = context.source_digest().to_string();
    for edits in [
        vec![edit("u1", "one", "changed"), edit("u2", "wrong", "changed")],
        vec![edit("u1", "one", "changed"), edit("u1", "changed", "again")],
        vec![edit("u999", "protected", "changed")],
    ] {
        assert!(context.commit(&source, 0, &edits).is_err());
        assert_eq!(context.request().unwrap(), raw);
        assert_eq!(context.version(), 0);
    }
    assert!(
        context
            .commit("another-source", 0, &[edit("u1", "one", "changed")])
            .is_err()
    );
    context
        .commit(&source, 0, &[edit("u1", "one", "changed")])
        .unwrap();
    assert!(
        context
            .commit(&source, 0, &[edit("u2", "two", "changed")])
            .is_err()
    );
    context
        .commit(&source, 1, &[edit("u2", "two", "second")])
        .unwrap();
    assert_eq!(
        context.request().unwrap()["messages"][0]["content"],
        "protected"
    );
    assert_eq!(context.version(), 2);
}

#[test]
fn anchors_cannot_be_forged_even_in_host_owned_mode() {
    let raw = json!({"model":"owned","n":2,"messages":[{"role":"system","content":"protected"},
        {"role":"user","content":"working"}]});
    let mut decoded = protocol::decode_host_owned("openai-chat", &raw).unwrap();
    assert_eq!(decoded.sidecar(), &raw);
    decoded.units[0].anchor = json!(["messages", 0, "content"]);
    assert!(protocol::encode(&decoded).is_err());
    let restored = NativeContext::restore("openai-chat", &raw, &raw, 4).unwrap();
    assert_eq!(restored.version(), 4);
    assert_eq!(restored.source_request(), &raw);
    let mut changed_control = raw.clone();
    changed_control["model"] = json!("forged-model");
    assert!(NativeContext::restore("openai-chat", &raw, &changed_control, 4).is_err());
}

const FIRST: &str = "event: native\nid: 7\ndata: {\"choices\":[{\"index\":1,\"delta\":{\"content\":\"second\"}},{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"arguments\":\"{\"}}]}}]}\n\n";
const LAST: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]},\"finish_reason\":\"tool_calls\"},{\"index\":1,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"total_tokens\":27}}\n\ndata: [DONE]\n\n";

#[derive(Clone)]
struct Fixture {
    observed: Arc<Mutex<Vec<Value>>>,
    release: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
}

async fn provider(State(state): State<Fixture>, Json(request): Json<Value>) -> Response {
    state.observed.lock().unwrap().push(request.clone());
    if request["stream"] == true {
        let gate = state.release.lock().unwrap().take().unwrap();
        let stream = futures_util::stream::unfold((0, Some(gate)), |(step, gate)| async move {
            match step {
                0 => Some((
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(FIRST.as_bytes())),
                    (1, gate),
                )),
                1 => {
                    gate.unwrap().await.unwrap();
                    Some((Ok(Bytes::from_static(LAST.as_bytes())), (2, None)))
                }
                _ => None,
            }
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("x-harness-route", "original-native-route")
            .body(Body::from_stream(stream))
            .unwrap()
    } else {
        Response::builder().status(StatusCode::OK).header("content-type","application/json")
            .body(Body::from(r#"{"choices":[{"index":1,"message":{"role":"assistant","content":"second"}},{"index":0,"message":{"role":"assistant","content":"first"}}],"opaque_output":{"keep":true}}"#)).unwrap()
    }
}

#[tokio::test]
async fn host_dispatch_preserves_native_stream_multiple_choices_and_all_request_options() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let (release, gate) = tokio::sync::oneshot::channel();
    let state = Fixture {
        observed: observed.clone(),
        release: Arc::new(Mutex::new(Some(gate))),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/native/chat/completions", post(provider))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    // Only this test's owned loopback listener is a possible dispatch target.
    let url = format!("http://{address}/native/chat/completions");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let raw = json!({"model":"harness-original-model","stream":true,"n":2,
        "stream_options":{"include_usage":true},"temperature":0.37,"max_completion_tokens":33,
        "seed":17,"reasoning_effort":"high","service_tier":"fixture","metadata":{"custom":"keep"},
        "tools":[{"type":"function","function":{"name":"fixture","strict":true,"parameters":{"type":"object"}}}],
        "messages":[{"role":"system","content":"protected"},{"role":"user","content":"before"}]});
    let mut context = NativeContext::new("openai-chat", &raw).unwrap();
    let source = context.source_digest().to_string();
    context
        .commit(
            &source,
            0,
            &[edit("u1", "before", "compressed actual context")],
        )
        .unwrap();
    let response = context
        .dispatch(|native| {
            // Harness policy/admission runs on edited native input here.
            assert_eq!(native["model"], "harness-original-model");
            assert_eq!(
                native["messages"][1]["content"],
                "compressed actual context"
            );
            let request = client.post(&url).json(&native);
            async move { request.send().await.map_err(|e| e.to_string()) }
        })
        .await
        .unwrap();
    assert_eq!(
        response.headers()["x-harness-route"],
        "original-native-route"
    );
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.bytes_stream();
    // The provider cannot emit its final chunk until the client has received
    // the first. A buffering/SSE reconstruction implementation deadlocks here.
    let first = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), FIRST.as_bytes());
    release.send(()).unwrap();
    let mut bytes = first.to_vec();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(bytes, format!("{FIRST}{LAST}").into_bytes());
    let mut expected = raw;
    expected["messages"][1]["content"] = json!("compressed actual context");
    assert_eq!(observed.lock().unwrap()[0], expected);

    expected["stream"] = json!(false);
    let context = NativeContext::new("openai-chat", &expected).unwrap();
    let response = context
        .dispatch(|native| {
            let request = client.post(&url).json(&native);
            async move { request.send().await.map_err(|e| e.to_string()) }
        })
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["choices"].as_array().unwrap().len(), 2);
    assert_eq!(body["choices"][0]["index"], 1);
    assert_eq!(body["opaque_output"]["keep"], true);
    assert_eq!(observed.lock().unwrap()[1], expected);
    server.abort();
}

#[tokio::test]
async fn responses_and_anthropic_use_their_original_native_routes_and_response_bytes() {
    for (format, endpoint, request, pointer, body) in [
        (
            "openai-responses",
            "/native/responses",
            json!({"model":"original/responses-model","stream":true,"input":"before",
                "reasoning":{"effort":"high"},"text":{"format":{"type":"json_object"}},
                "previous_response_id":"previous","tools":[{"type":"web_search"}]}),
            "/input",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":1,\"delta\":\"native\"}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ),
        (
            "anthropic-messages",
            "/native/messages",
            json!({"model":"original/anthropic-model","stream":true,"max_tokens":2000,
                "system":[{"type":"text","text":"protected","cache_control":{"type":"ephemeral"}}],
                "thinking":{"type":"enabled","budget_tokens":1024},
                "messages":[{"role":"user","content":[{"type":"text","text":"before"}]}]}),
            "/messages/0/content/0/text",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"native\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
    ] {
        let observed = Arc::new(Mutex::new(None));
        let captured = observed.clone();
        let app = Router::new().route(
            endpoint,
            post(move |Json(native): Json<Value>| {
                let captured = captured.clone();
                async move {
                    *captured.lock().unwrap() = Some(native);
                    Response::builder()
                        .status(202)
                        .header("content-type", "text/event-stream")
                        .header("x-native-format", format)
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{endpoint}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut context = NativeContext::new(format, &request).unwrap();
        let source = context.source_digest().to_owned();
        context
            .commit(&source, 0, &[edit("u1", "before", "edited context")])
            .unwrap();
        let response = context
            .dispatch(|native| {
                let request = client.post(&url).json(&native);
                async move { request.send().await.map_err(|e| e.to_string()) }
            })
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 202);
        assert_eq!(response.headers()["x-native-format"], format);
        assert_eq!(response.bytes().await.unwrap().as_ref(), body.as_bytes());
        let mut expected = request;
        *expected.pointer_mut(pointer).unwrap() = json!("edited context");
        assert_eq!(observed.lock().unwrap().as_ref().unwrap(), &expected);
        server.abort();
    }
}
