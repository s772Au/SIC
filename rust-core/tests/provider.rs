mod model_policy {
    pub use sic_core_rust::model_policy::*;
}

#[path = "../src/provider.rs"]
mod provider;

use provider::{
    CallRole, FrozenInputCount, chat_completion, chat_completion_observed, validate_openai_profile,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const TEST_KEY: &str = "fixture-key-do-not-print";

fn profile(api_base: String) -> Value {
    json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "expected_returned_models": ["deepseek-v4.1-flash-eco"],
        "api_base": api_base,
        "api_key_env": "SIC_TEST_PROVIDER_KEY",
        "tokenizer_model": "host-counter-gpt4o",
        "tokenizer_status": "host-selected-surrogate-not-provider-attested",
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 393_216,
        "default_output_tokens": 16_384,
        "max_supported_output_tokens": 393_216,
        "context_window": 1_000_000,
        "timeout_seconds": 5
    })
}

fn messages() -> Value {
    json!([
        {"role":"system","content":"Treat this as a fixture."},
        {"role":"user","content":"Return a short answer."}
    ])
}

fn tools() -> Value {
    json!([
        {"type":"function","function":{"name":"sic_source_read","description":"Read authorized source text.","parameters":{"type":"object","properties":{"source_id":{"type":"string"}},"required":["source_id"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"sic_edit_blocks","description":"Deliver an Editor response.","parameters":{"type":"object","properties":{"ops":{"type":"array"},"pvi":{"type":"object"}},"required":["ops","pvi"],"additionalProperties":false}}}
    ])
}

fn request(messages: Value, reasoning_effort: Option<&str>) -> Value {
    let mut request = json!({"messages":messages,"tools":tools(),"tool_choice":"auto"});
    if let Some(effort) = reasoning_effort {
        request["reasoning_effort"] = json!(effort);
    }
    request
}

fn frozen(tokens: u64) -> FrozenInputCount {
    FrozenInputCount {
        tokens,
        counter_id: "host-fixture-counter-v1".into(),
    }
}

async fn fixture(response: String) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut header_end = None;
        let mut content_length = 0usize;
        loop {
            let mut buffer = [0u8; 2048];
            let size = socket.read(&mut buffer).await.unwrap();
            assert!(
                size > 0,
                "fixture request ended before complete headers/body"
            );
            request.extend_from_slice(&buffer[..size]);
            if header_end.is_none() {
                if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let end = position + 4;
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    header_end = Some(end);
                }
            }
            if header_end.is_some_and(|end| request.len() >= end + content_length) {
                break;
            }
        }
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request).to_string()
    });
    (format!("http://{address}/v1"), task)
}

fn response(status: &str, body: &str, extra_headers: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{}",
        body.len(),
        body
    )
}

fn ok_body(with_usage: bool) -> String {
    let mut body = json!({
        "id":"fixture-response",
        "object":"chat.completion",
        "model":"deepseek-v4.1-flash-eco",
        "choices":[{"index":0,"message":{"role":"assistant","content":"fixture answer"},"finish_reason":"stop"}]
    });
    if with_usage {
        body["usage"] = json!({"prompt_tokens":12,"completion_tokens":3,"total_tokens":15});
    }
    body.to_string()
}

#[tokio::test]
async fn reasoning_efforts_are_forwarded_without_implicit_mapping() {
    for effort in crate::model_policy::REASONING_EFFORTS {
        let (base, server) = fixture(response("200 OK", &ok_body(false), "")).await;
        let p = profile(base);
        chat_completion(
            &p,
            &request(messages(), Some(effort)),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap();
        let wire = server.await.unwrap();
        let body = wire.split_once("\r\n\r\n").unwrap().1;
        let sent: Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent["reasoning_effort"], effort);
    }
}

#[test]
fn validates_backend_example_profile_without_reading_credentials() {
    let p = profile("https://provider.invalid/v1".into());
    assert!(validate_openai_profile(&p).is_ok());
    assert_eq!(p["api_key_env"], "SIC_TEST_PROVIDER_KEY");
    assert_eq!(p["context_window"], 1_000_000);
    assert_eq!(p["max_output_tokens"], 393_216);
    let example: Value =
        serde_json::from_str(include_str!("../config/opendesign.example.json")).unwrap();
    let configured = &example["profiles"]["opendesign-flash-eco"];
    assert!(validate_openai_profile(configured).is_ok());
    assert_eq!(
        configured["credential_origin"],
        "https://amr-link.open-design.ai"
    );
    assert_eq!(configured["retry_editor_http_5xx"], true);
    let mut invalid = configured.clone();
    invalid["retry_editor_http_5xx"] = json!("true");
    assert!(validate_openai_profile(&invalid).is_err());
    invalid = configured.clone();
    invalid["retry_worker_http_5xx"] = json!("true");
    assert!(validate_openai_profile(&invalid).is_err());
    invalid["retry_worker_http_5xx"] = json!(true);
    assert!(validate_openai_profile(&invalid).is_ok());
}

#[test]
fn worker_retry_policy_defaults_and_bounds_are_validated() {
    let base = profile("https://provider.invalid/v1".into());
    assert_eq!(
        crate::model_policy::worker_retry_policy(&base).unwrap(),
        (2, 250),
        "omitted keys keep the historical one-retry/250 ms policy"
    );
    for (key, bad) in [
        ("worker_retry_max_attempts", json!(0)),
        ("worker_retry_max_attempts", json!(6)),
        ("worker_retry_max_attempts", json!("3")),
        ("worker_retry_backoff_ms", json!(5001)),
        ("worker_retry_backoff_ms", json!(-1)),
    ] {
        let mut candidate = base.clone();
        candidate[key] = bad;
        assert!(
            validate_openai_profile(&candidate).is_err(),
            "{key} accepted an invalid value"
        );
    }
    for (key, good, expected) in [
        ("worker_retry_max_attempts", json!(1), (1u32, 250u64)),
        ("worker_retry_max_attempts", json!(5), (5u32, 250u64)),
        ("worker_retry_backoff_ms", json!(0), (2u32, 0u64)),
        ("worker_retry_backoff_ms", json!(5000), (2u32, 5000u64)),
    ] {
        let mut candidate = base.clone();
        candidate[key] = good;
        assert!(validate_openai_profile(&candidate).is_ok());
        assert_eq!(
            crate::model_policy::worker_retry_policy(&candidate).unwrap(),
            expected
        );
    }
}

#[test]
fn rejects_url_credentials_query_fragment_and_remote_http() {
    for base in [
        "https://user:pass@example.invalid/v1",
        "https://example.invalid/v1?model=x",
        "https://example.invalid/v1#frag",
        "http://example.invalid/v1",
    ] {
        assert!(validate_openai_profile(&profile(base.into())).is_err());
    }
    assert!(validate_openai_profile(&profile("http://127.0.0.1:1234/v1".into())).is_ok());
}

#[test]
fn credential_origin_pins_scheme_host_and_port_without_path() {
    let mut p = profile("https://amr-link.open-design.ai/v1".into());
    p["credential_origin"] = json!("https://amr-link.open-design.ai");
    assert!(validate_openai_profile(&p).is_ok());
    // A trailing slash and an explicit default port represent the same origin.
    p["credential_origin"] = json!("https://amr-link.open-design.ai:443/");
    assert!(validate_openai_profile(&p).is_ok());
    for pinned in [
        json!("https://api.deepseek.com"),
        json!("https://amr-link.open-design.ai.attacker.invalid"),
        json!("https://amr-link.open-design.ai:8443"),
        json!("http://amr-link.open-design.ai"),
        json!("https://amr-link.open-design.ai/v1"),
        json!("https://amr-link.open-design.ai?query=1"),
        json!(""),
        json!(null),
    ] {
        p["credential_origin"] = pinned;
        assert!(validate_openai_profile(&p).is_err());
    }
}

#[tokio::test]
async fn mismatched_origin_is_rejected_before_resolving_credential() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut p = profile("https://api.deepseek.com/v1".into());
    p["credential_origin"] = json!("https://amr-link.open-design.ai");
    let resolved = AtomicBool::new(false);
    let error = chat_completion(
        &p,
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| {
            resolved.store(true, Ordering::SeqCst);
            Ok(TEST_KEY.into())
        },
    )
    .await
    .unwrap_err();
    assert!(error.contains("credential_origin"));
    assert!(!resolved.load(Ordering::SeqCst));
}

#[tokio::test]
async fn matching_origin_allows_the_intended_local_provider() {
    let (base, server) = fixture(response("200 OK", &ok_body(false), "")).await;
    let mut p = profile(base.clone());
    p["credential_origin"] = json!(base.strip_suffix("/v1").unwrap());
    let result = chat_completion(
        &p,
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    assert_eq!(result.returned_model, "deepseek-v4.1-flash-eco");
    let _ = server.await.unwrap();
}

#[test]
fn rejects_inline_key() {
    let mut p = profile("http://localhost:1234/v1".into());
    p["api_key"] = json!("secret-present-but-not-read");
    assert!(validate_openai_profile(&p).is_err());
}

#[tokio::test]
async fn sends_one_fixed_text_request_and_returns_raw_response_and_usage() {
    let body = ok_body(true);
    let (base, server) = fixture(response("200 OK", &body, "")).await;
    let p = profile(base);
    let request = request(messages(), Some("xhigh"));
    let result = chat_completion(&p, &request, CallRole::Worker, &frozen(12), |_| {
        Ok(TEST_KEY.into())
    })
    .await
    .unwrap();
    let request = server.await.unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-key-do-not-print")
    );
    let (head, request_body) = request.split_once("\r\n\r\n").unwrap();
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    let sent: Value = serde_json::from_str(request_body).unwrap();
    assert_eq!(sent["model"], "deepseek-v4.1-flash-eco");
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["n"], 1);
    assert_eq!(sent["max_tokens"], 16_384);
    assert_eq!(sent["reasoning_effort"], "xhigh");
    assert_eq!(sent["tool_choice"], "auto");
    assert_eq!(sent["tools"], tools());
    assert_eq!(result.requested_model, "openai/deepseek-v4.1-flash-eco");
    assert_eq!(result.returned_model, "deepseek-v4.1-flash-eco");
    assert_eq!(result.finish_reason, "stop");
    assert_eq!(result.usage.as_ref().unwrap()["total_tokens"], 15);
    assert_eq!(result.admission.input_counter_id, "host-fixture-counter-v1");
    assert_eq!(result.admission.tokenizer_model, "host-counter-gpt4o");
    assert_eq!(
        result.admission.tokenizer_status,
        "host-selected-surrogate-not-provider-attested"
    );
    assert!(!result.raw_body.is_empty());
    assert!(!format!("{result:?}").contains(TEST_KEY));
}

#[tokio::test]
async fn editor_tool_response_is_returned_with_original_call_id_and_arguments() {
    let calls = json!([{
        "index":0,
        "id":"call_editor_01",
        "type":"function",
        "function":{"name":"sic_edit_blocks","arguments":"{\"ops\":[{\"op\":\"KEEP\",\"target\":\"wb-1\"}],\"pvi\":{}}"}
    }]);
    let body = json!({
        "id":"fixture-editor-response",
        "object":"chat.completion",
        "model":"deepseek-v4.1-flash-eco",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":calls},"finish_reason":"tool_calls"}]
    });
    let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
    let result = chat_completion(
        &profile(base),
        &request(messages(), Some("high")),
        CallRole::Editor,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    let sent = server.await.unwrap();
    assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert_eq!(result.finish_reason, "tool_calls");
    assert_eq!(
        result.tool_calls.as_ref().unwrap()[0]["id"],
        "call_editor_01"
    );
    assert_eq!(result.tool_calls.as_ref().unwrap()[0]["index"], 0);
    assert_eq!(
        result.tool_calls.as_ref().unwrap()[0]["function"]["arguments"],
        "{\"ops\":[{\"op\":\"KEEP\",\"target\":\"wb-1\"}],\"pvi\":{}}"
    );
}

#[tokio::test]
async fn completed_parallel_calls_normalize_provider_indexes_but_preserve_raw_response() {
    let calls = json!([
        {"index":0,"id":"read-a","type":"function","function":{"name":"sic_source_read","arguments":"{\"source_id\":\"a\"}"}},
        {"index":0,"id":"read-b","type":"function","function":{"name":"sic_source_read","arguments":"{\"source_id\":\"b\"}"}}
    ]);
    let body = json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls",
        "message":{"role":"assistant","content":null,"tool_calls":calls}}]});
    let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
    let result = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    let _ = server.await.unwrap();
    let normalized = result.tool_calls.unwrap();
    assert_eq!(normalized[0]["index"], 0);
    assert_eq!(normalized[1]["index"], 1);
    assert_eq!(normalized[1]["id"], "read-b");
    assert_eq!(
        normalized[1]["function"]["arguments"],
        "{\"source_id\":\"b\"}"
    );
    assert_eq!(
        result.raw_response["choices"][0]["message"]["tool_calls"],
        calls
    );
}

#[tokio::test]
async fn parallel_call_normalization_still_rejects_duplicate_ids_and_invalid_index_types() {
    let valid = json!({"index":0,"id":"read-a","type":"function",
        "function":{"name":"sic_source_read","arguments":"{\"source_id\":\"a\"}"}});
    for bad in [
        json!({"index":0,"id":"read-a","type":"function","function":{"name":"sic_source_read","arguments":"{}"}}),
        json!({"index":-1,"id":"read-b","type":"function","function":{"name":"sic_source_read","arguments":"{}"}}),
        json!({"index":"0","id":"read-b","type":"function","function":{"name":"sic_source_read","arguments":"{}"}}),
    ] {
        let body = json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls",
            "message":{"role":"assistant","content":null,"tool_calls":[valid.clone(),bad]}}]});
        let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
        let result = chat_completion(
            &profile(base),
            &request(messages(), None),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await;
        assert!(result.is_err());
        let _ = server.await.unwrap();
    }
}

#[tokio::test]
async fn assistant_tool_call_and_tool_result_keep_native_ids_and_arguments() {
    let call_args = "{\"source_id\":\"src-1\"}";
    let history = json!([
        {"role":"user","content":"Read the source."},
        {"role":"assistant","content":null,"reasoning_content":"opaque synthetic reasoning","tool_calls":[{"id":"call_read_01","type":"function","function":{"name":"sic_source_read","arguments":call_args}}]},
        {"role":"tool","tool_call_id":"call_read_01","name":"sic_source_read","content":"source text"},
        {"role":"user","content":"Now answer."}
    ]);
    let (base, server) = fixture(response("200 OK", &ok_body(false), "")).await;
    let expected = request(history.clone(), None);
    let _result = chat_completion(
        &profile(base),
        &expected,
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    let sent = server.await.unwrap();
    let (_, body) = sent.split_once("\r\n\r\n").unwrap();
    let sent_json: Value = serde_json::from_str(body).unwrap();
    assert_eq!(sent_json["messages"], history);
    assert_eq!(
        sent_json["messages"][1]["tool_calls"][0]["id"],
        "call_read_01"
    );
    assert_eq!(
        sent_json["messages"][1]["tool_calls"][0]["function"]["arguments"],
        call_args
    );
    assert_eq!(sent_json["messages"][2]["tool_call_id"], "call_read_01");
    assert_eq!(
        sent_json["messages"][1]["reasoning_content"],
        "opaque synthetic reasoning"
    );
}

#[tokio::test]
async fn worker_can_request_a_registered_native_tool() {
    let call = json!({
        "id":"call_worker_09",
        "type":"function",
        "function":{"name":"sic_source_read","arguments":"{\"source_id\":\"src-9\"}"}
    });
    let body = json!({
        "model":"deepseek-v4.1-flash-eco",
        "choices":[{"message":{"role":"assistant","content":null,"tool_calls":[call]},"finish_reason":"tool_calls"}]
    });
    let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
    let result = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    let _ = server.await.unwrap();
    assert_eq!(
        result.tool_calls.as_ref().unwrap()[0]["id"],
        "call_worker_09"
    );
    assert_eq!(
        result.tool_calls.as_ref().unwrap()[0]["function"]["name"],
        "sic_source_read"
    );
}

#[tokio::test]
async fn rejects_malformed_tool_calls_and_text_only_editor_delivery() {
    let malformed = [
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"message":{"role":"assistant","tool_calls":[{"type":"function","function":{"name":"sic_edit_blocks","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call_bad","type":"function","function":{"name":"sic_edit_blocks","arguments":"{\"ops\":"}}]},"finish_reason":"tool_calls"}]}),
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"message":{"role":"user","tool_calls":[{"id":"call_bad","type":"function","function":{"name":"sic_edit_blocks","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
    ];
    for body in malformed {
        let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
        let err = chat_completion(
            &profile(base),
            &request(messages(), None),
            CallRole::Editor,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap_err();
        let _ = server.await.unwrap();
        assert!(err.contains("tool call") || err.contains("role"));
    }

    let (base, server) = fixture(response("200 OK", &ok_body(false), "")).await;
    let err = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Editor,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap_err();
    let _ = server.await.unwrap();
    assert!(err.contains("sic_edit_blocks"));
}

#[tokio::test]
async fn rejected_native_tool_output_is_observable_for_durable_audit() {
    let body=json!({"model":"deepseek-v4.1-flash-eco","choices":[{"finish_reason":"tool_calls","message":{
        "role":"assistant","tool_calls":[{"id":"bad","index":0,"type":"function","function":{
            "name":"sic_edit_blocks","arguments":"{incomplete"}}]}}]}).to_string();
    let (base, server) = fixture(response("200 OK", &body, "")).await;
    let saved = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let target = saved.clone();
    let error = chat_completion_observed(
        &profile(base),
        &request(messages(), None),
        CallRole::Editor,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
        move |raw| target.lock().unwrap().extend_from_slice(raw),
    )
    .await
    .unwrap_err();
    assert!(error.contains("arguments"));
    assert_eq!(*saved.lock().unwrap(), body.as_bytes());
    server.await.unwrap();
}

#[tokio::test]
async fn rejects_request_overrides_and_non_auto_tool_choice_before_secret_resolution() {
    let p = profile("http://127.0.0.1:1/v1".into());
    let called = std::sync::atomic::AtomicBool::new(false);
    for (key, value) in [
        ("model", json!("attacker/override")),
        ("api_base", json!("https://attacker.invalid/v1")),
        ("api_key_env", json!("ATTACKER_KEY")),
        ("max_tokens", json!(999_999)),
        ("max_input_tokens", json!(1)),
        ("context_window", json!(1)),
    ] {
        let mut bad = request(messages(), None);
        bad[key] = value;
        let error = chat_completion(&p, &bad, CallRole::Worker, &frozen(12), |_| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(TEST_KEY.into())
        })
        .await
        .unwrap_err();
        assert!(error.contains("unsupported Chat control"));
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    let mut bad = request(messages(), None);
    bad["tool_choice"] = json!("required");
    assert!(
        chat_completion(&p, &bad, CallRole::Worker, &frozen(12), |_| Ok(
            TEST_KEY.into()
        ))
        .await
        .unwrap_err()
        .contains("fixed to auto")
    );
    bad["tool_choice"] = Value::Null;
    assert!(
        chat_completion(&p, &bad, CallRole::Worker, &frozen(12), |_| Ok(
            TEST_KEY.into()
        ))
        .await
        .unwrap_err()
        .contains("tool_choice")
    );
}

#[tokio::test]
async fn passes_through_all_five_reasoning_efforts() {
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        let (base, server) = fixture(response("200 OK", &ok_body(false), "")).await;
        let _result = chat_completion(
            &profile(base),
            &request(messages(), Some(effort)),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        let sent: Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent["reasoning_effort"], effort);
    }
}

#[tokio::test]
async fn absent_provider_usage_stays_unknown() {
    let body = ok_body(false);
    let (base, server) = fixture(response("200 OK", &body, "")).await;
    let result = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap();
    let _request = server.await.unwrap();
    assert_eq!(result.usage, None);
}

#[tokio::test]
async fn enforces_frozen_input_output_and_combined_context_before_secret_resolution() {
    let (base, _server) = fixture(response("200 OK", &ok_body(true), "")).await;
    let p = profile(base);
    let called = std::sync::atomic::AtomicBool::new(false);
    let err = chat_completion(
        &p,
        &request(messages(), None),
        CallRole::Worker,
        &frozen(1_000_001),
        |_| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(TEST_KEY.into())
        },
    )
    .await
    .unwrap_err();
    assert!(err.contains("max_input_tokens"));
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));

    let mut p = profile("http://127.0.0.1:1/v1".into());
    p["default_output_tokens"] = json!(393_216);
    assert!(
        chat_completion(
            &p,
            &request(messages(), None),
            CallRole::Worker,
            &frozen(700_000),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap_err()
        .contains("context_window")
    );
}

#[tokio::test]
async fn rejects_429_without_retrying() {
    let (base, server) = fixture(response("429 Too Many Requests", "{}", "")).await;
    let error = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap_err();
    let request = server.await.unwrap();
    assert!(error.contains("429"));
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
}

#[tokio::test]
async fn rejects_redirect_instead_of_following_it() {
    let (base, server) = fixture(response(
        "302 Found",
        "{}",
        "Location: http://127.0.0.1/other\r\n",
    ))
    .await;
    let error = chat_completion(
        &profile(base),
        &request(messages(), None),
        CallRole::Worker,
        &frozen(12),
        |_| Ok(TEST_KEY.into()),
    )
    .await
    .unwrap_err();
    let request = server.await.unwrap();
    assert!(error.contains("redirect"));
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
}

#[tokio::test]
async fn rejects_truncated_unknown_and_wrong_model_responses() {
    let mut bads = vec![
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]}),
        json!({"model":"deepseek-v4.1-flash-eco","choices":[{"message":{"role":"assistant","content":"maybe"}}]}),
        json!({"model":"other-model","choices":[{"message":{"role":"assistant","content":"no"},"finish_reason":"stop"}]}),
    ];
    let expected_errors = ["truncated", "completion status", "model identity"];
    for (body, needle) in bads.drain(..).zip(expected_errors) {
        let (base, server) = fixture(response("200 OK", &body.to_string(), "")).await;
        let error = chat_completion(
            &profile(base),
            &request(messages(), None),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap_err();
        let _request = server.await.unwrap();
        assert!(error.contains(needle));
    }
}

#[tokio::test]
async fn rejects_non_text_messages_and_unknown_reasoning_effort_before_network() {
    let p = profile("http://127.0.0.1:1/v1".into());
    assert!(
        chat_completion(
            &p,
            &request(
                json!([{"role":"user","content":[{"type":"image_url"}]}]),
                None
            ),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into())
        )
        .await
        .unwrap_err()
        .contains("plain-text")
    );
    assert!(
        chat_completion(
            &p,
            &request(messages(), Some("unrecognized")),
            CallRole::Worker,
            &frozen(12),
            |_| Ok(TEST_KEY.into()),
        )
        .await
        .unwrap_err()
        .contains("reasoning_effort")
    );
}
