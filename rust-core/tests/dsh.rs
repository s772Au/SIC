mod model_policy {
    pub use sic_core_rust::model_policy::*;
}

#[path = "../src/dsh.rs"]
mod dsh;

use axum::http::{HeaderMap, HeaderValue};
use dsh::{DshChatRequest, DshSessionHistory, buffered_sse};
use serde_json::{Value, json};

fn headers(session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-sic-bridge-session",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

fn tool_schema() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "sic_edit_blocks",
            "description": "Apply SIC edit blocks",
            "parameters": {
                "type": "object",
                "properties": {"blocks": {"type": "array"}},
                "required": ["blocks"],
                "additionalProperties": false
            }
        }
    }])
}

fn request(messages: Value) -> Value {
    json!({
        "model": "sic-worker",
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
        "tools": tool_schema(),
        "tool_choice": "auto",
        "max_tokens": 2048,
        "reasoning_effort": "high"
    })
}

fn parse(session: &str, body: &Value) -> DshChatRequest {
    DshChatRequest::parse(&headers(session), body, "sic-worker").unwrap()
}

#[test]
fn accepts_reasoning_mode_and_compatibility_efforts_without_mapping_them() {
    for effort in [
        "none", "minimal", "low", "medium", "high", "xhigh", "ultra", "max",
    ] {
        let mut body = request(json!([{"role":"user","content":"x"}]));
        body["reasoning_effort"] = json!(effort);
        assert_eq!(
            parse("sess-effort", &body).reasoning_effort.as_deref(),
            Some(effort)
        );
    }
    let mut invalid = request(json!([{"role":"user","content":"x"}]));
    invalid["reasoning_effort"] = json!("unrecognized");
    assert!(DshChatRequest::parse(&headers("sess-effort"), &invalid, "sic-worker").is_err());
}

#[test]
fn parses_native_dsh_request_and_keeps_public_tools_and_history() {
    let messages = json!([
        {"role":"system","content":"You are SIC."},
        {"role":"user","content":"Change the title."}
    ]);
    let parsed = parse("sess-a", &request(messages.clone()));
    assert_eq!(parsed.session_id, "sess-a");
    assert_eq!(parsed.model_alias, "sic-worker");
    assert_eq!(parsed.messages, messages.as_array().unwrap().clone());
    assert_eq!(parsed.tools, tool_schema());
    assert_eq!(parsed.tool_choice, "auto");
    assert!(parsed.include_usage);
    assert_eq!(parsed.max_tokens, Some(2048));
    assert_eq!(parsed.reasoning_effort.as_deref(), Some("high"));
    let provider_request = parsed.provider_request();
    assert_eq!(provider_request["tools"], tool_schema());
    assert_eq!(provider_request["messages"], messages);
    assert!(provider_request.get("model").is_none());
    assert!(provider_request.get("stream").is_none());
    assert!(provider_request.get("max_tokens").is_none());
}

#[test]
fn caps_only_output_budget_and_does_not_raise_profile_limit() {
    let parsed = parse("sess-a", &request(json!([{"role":"user","content":"x"}])));
    let profile = json!({
        "model":"deepseek-v4.1-flash-eco",
        "api_base":"https://provider.example/v1",
        "api_key_env":"SIC_KEY",
        "max_output_tokens":4096,
        "default_output_tokens":3000
    });
    assert_eq!(
        parsed.profile_with_output_cap(&profile).unwrap()["default_output_tokens"],
        2048
    );
    let too_high = json!({"max_output_tokens":1024});
    assert!(parsed.profile_with_output_cap(&too_high).is_err());
}

#[test]
fn requires_session_and_fixed_route_and_rejects_unexpressible_controls() {
    let body = request(json!([{"role":"user","content":"x"}]));
    assert!(DshChatRequest::parse(&HeaderMap::new(), &body, "sic-worker").is_err());
    assert!(DshChatRequest::parse(&headers("s"), &body, "other-model").is_err());

    for (key, value) in [
        ("stream", json!(false)),
        ("store", json!(true)),
        ("temperature", json!(0.2)),
        ("tool_choice", json!("required")),
        ("reasoning_effort", json!("unrecognized")),
        ("unknown_control", json!(true)),
    ] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().insert(key.into(), value);
        assert!(
            DshChatRequest::parse(&headers("s"), &invalid, "sic-worker").is_err(),
            "accepted {key}"
        );
    }

    let mut images = body.clone();
    images["messages"][0]["content"] =
        json!([{"type":"image_url","image_url":{"url":"https://example.invalid/x"}}]);
    assert!(DshChatRequest::parse(&headers("s"), &images, "sic-worker").is_err());
}

#[test]
fn full_history_cursor_preserves_tool_call_ids_and_accepts_json_reserialization() {
    let first_messages = json!([
        {"role":"system","content":"system"},
        {"role":"user","content":"edit"}
    ]);
    let first = parse("sess", &request(first_messages));
    let mut history = DshSessionHistory::default();
    assert_eq!(
        history.new_messages(&first).unwrap(),
        vec![
            json!({"role":"system","content":"system"}),
            json!({"role":"user","content":"edit"})
        ]
    );
    let generated = json!({
        "role":"assistant",
        "content":null,
        "tool_calls":[{"id":"call-1","type":"function","function":{"name":"sic_edit_blocks","arguments":"{\"blocks\":[{\"text\":\"ok\"}]}"}}]
    });
    history.commit_reply(&first, generated.clone()).unwrap();

    let second_messages = json!([
        {"role":"system","content":"system"},
        {"role":"user","content":"edit"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"sic_edit_blocks","arguments":"{ \"blocks\" : [ { \"text\" : \"ok\" } ] }"}}]},
        {"role":"tool","tool_call_id":"call-1","name":"sic_edit_blocks","content":"applied"},
        {"role":"user","content":"thanks"}
    ]);
    let second = parse("sess", &request(second_messages.clone()));
    assert_eq!(
        history.new_messages(&second).unwrap(),
        vec![second_messages[3].clone(), second_messages[4].clone()]
    );

    let mut changed = second.clone();
    changed.messages[2]["tool_calls"][0]["id"] = json!("call-forged");
    assert!(history.new_messages(&changed).is_err());

    let mut truncated = second.clone();
    truncated.messages.remove(0);
    assert!(history.new_messages(&truncated).is_err());
}

#[test]
fn tool_call_reply_is_buffered_as_pi_ai_sse_with_original_id_arguments_and_usage() {
    let reply = json!({
        "id":"chatcmpl-test",
        "created":1700000000,
        "model":"deepseek-v4.1-flash-eco",
        "choices":[{"index":0,"finish_reason":"tool_calls","message":{
            "role":"assistant",
            "content":null,
            "tool_calls":[{"id":"call-edit-7","type":"function","function":{"name":"sic_edit_blocks","arguments":"{\"blocks\":[{\"op\":\"replace\"}]}"}}]
        }}],
        "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}
    });
    let sse = String::from_utf8(buffered_sse(&reply, true).unwrap()).unwrap();
    let frames: Vec<Value> = sse
        .split("\n\n")
        .filter_map(|frame| frame.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames[0]["choices"][0]["delta"]["tool_calls"][0]["id"],
        "call-edit-7"
    );
    assert_eq!(
        frames[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
        0
    );
    assert_eq!(
        frames[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        "{\"blocks\":[{\"op\":\"replace\"}]}"
    );
    assert_eq!(frames[1]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(frames[2]["choices"], json!([]));
    assert_eq!(frames[2]["usage"]["total_tokens"], 15);
    assert!(sse.ends_with("data: [DONE]\n\n"));
}

#[test]
fn rejects_truncated_or_malformed_provider_tool_calls() {
    let base = json!({
        "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call-x","type":"function","function":{"name":"sic_edit_blocks","arguments":"{\"blocks\":"}}]}}]
    });
    assert!(buffered_sse(&base, true).is_err());

    let no_id = json!({
        "choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,
            "tool_calls":[{"type":"function","function":{"name":"sic_edit_blocks","arguments":"{}"}}]}}]
    });
    assert!(buffered_sse(&no_id, false).is_err());

    let unknown_finish = json!({
        "choices":[{"finish_reason":null,"message":{"role":"assistant","content":"partial"}}]
    });
    assert!(buffered_sse(&unknown_finish, false).is_err());
}

#[test]
fn omits_missing_usage_and_preserves_text_finish() {
    let reply = json!({
        "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]
    });
    let sse = String::from_utf8(buffered_sse(&reply, true).unwrap()).unwrap();
    assert!(!sse.contains("\"usage\""));
    assert!(sse.contains("\"finish_reason\":\"stop\""));
    assert!(sse.ends_with("data: [DONE]\n\n"));
}
