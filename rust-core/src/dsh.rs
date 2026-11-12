//! DSH headless OpenAI Chat Completions protocol adapter.
//!
//! The installed DSH PiAi adapter posts full Chat history to `/v1/chat/completions`
//! with `x-sic-bridge-session`, `stream=true`, function tools and optional usage.
//! This module validates that envelope, preserves native calls/results, and can
//! encode a buffered Chat reply as the SSE frames consumed by PiAi. It is a
//! protocol adapter only: callers must mount it in an authenticated host,
//! serialize/persist session state, run Core/Editor orchestration, and register
//! any SIC tools with DSH before claiming a live host integration.

use axum::http::HeaderMap;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use uuid::Uuid;

const SESSION_HEADERS: [&str; 4] = [
    "x-sic-bridge-session",
    "session_id",
    "x-session-id",
    "x-session-affinity",
];
const REQUEST_FIELDS: [&str; 11] = [
    "model",
    "messages",
    "stream",
    "stream_options",
    "tools",
    "tool_choice",
    "max_tokens",
    "temperature",
    "reasoning_effort",
    "n",
    "store",
];

#[derive(Clone, Debug, PartialEq)]
pub struct DshChatRequest {
    pub session_id: String,
    pub model_alias: String,
    /// Exact full native message history sent by DSH.
    pub messages: Vec<Value>,
    /// Exact host-visible OpenAI function schema table, if supplied.
    pub tools: Value,
    pub tool_choice: String,
    pub include_usage: bool,
    pub max_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
}

impl DshChatRequest {
    /// Parse a real DSH PiAi Chat request without rewriting history or tools.
    /// Only the explicit session header, expected public model alias, and the
    /// small OpenAI Chat subset below are admitted.
    pub fn parse(
        headers: &HeaderMap,
        body: &Value,
        expected_model_alias: &str,
    ) -> Result<Self, String> {
        if expected_model_alias.trim().is_empty() {
            return Err("configured DSH model alias must be nonempty".into());
        }
        let session_id = session_identity(headers)?;
        let object = body
            .as_object()
            .ok_or_else(|| "DSH Chat request must be a JSON object".to_string())?;
        if object
            .keys()
            .any(|key| !REQUEST_FIELDS.contains(&key.as_str()))
        {
            return Err("DSH request contains unsupported or unsafe control fields".into());
        }
        let model_alias = body
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty())
            .ok_or_else(|| "DSH Chat request requires its configured model alias".to_string())?;
        if model_alias != expected_model_alias {
            return Err("DSH model alias does not match the fixed host route".into());
        }
        if body.get("stream").and_then(Value::as_bool) != Some(true) {
            return Err("DSH PiAi route requires stream=true".into());
        }
        if let Some(n) = body.get("n") {
            if n.as_u64() != Some(1) {
                return Err("only one DSH Chat choice is supported".into());
            }
        }
        if body
            .get("store")
            .is_some_and(|store| store != &Value::Bool(false))
        {
            return Err("provider store must remain disabled".into());
        }
        if body.get("temperature").is_some() {
            return Err("temperature is not supported by the fixed SIC request path".into());
        }
        let include_usage = match body.get("stream_options") {
            None => false,
            Some(options) => {
                let options = options
                    .as_object()
                    .ok_or_else(|| "stream_options must be an object".to_string())?;
                if options.keys().any(|key| key != "include_usage") {
                    return Err("unsupported DSH stream option".into());
                }
                options
                    .get("include_usage")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| "stream_options.include_usage must be boolean".to_string())?
            }
        };
        if body.get("max_tokens").is_some()
            && body.get("max_tokens").and_then(Value::as_u64).is_none()
        {
            return Err("max_tokens must be a positive integer".into());
        }
        let max_tokens = body
            .get("max_tokens")
            .and_then(Value::as_u64)
            .filter(|limit| *limit > 0);
        if body.get("max_tokens").is_some() && max_tokens.is_none() {
            return Err("max_tokens must be a positive integer".into());
        }
        let tool_choice = match body.get("tool_choice") {
            None => "auto",
            Some(value) if value.as_str() == Some("auto") => "auto",
            Some(_) => return Err("only tool_choice=auto is supported".into()),
        }
        .to_owned();
        let reasoning_effort = match body.get("reasoning_effort") {
            None => None,
            Some(value) => {
                let effort = value
                    .as_str()
                    .filter(|effort| crate::model_policy::REASONING_EFFORTS.contains(effort))
                    .ok_or_else(|| "unsupported DSH reasoning_effort".to_string())?;
                Some(effort.to_owned())
            }
        };
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .filter(|messages| !messages.is_empty())
            .ok_or_else(|| "DSH request requires nonempty full messages history".to_string())?
            .clone();
        validate_history_messages(&messages)?;
        let tools = body.get("tools").cloned().unwrap_or_else(|| json!([]));
        if !tools.is_array() {
            return Err("DSH tools must be an array".into());
        }
        Ok(Self {
            session_id,
            model_alias: model_alias.to_owned(),
            messages,
            tools,
            tool_choice,
            include_usage,
            max_tokens,
            reasoning_effort,
        })
    }

    /// Return the native provider Chat request fields accepted by the fixed
    /// provider client. Profile identity, endpoint, credentials and budgets are
    /// intentionally absent from this value.
    pub fn provider_request(&self) -> Value {
        let mut request = json!({
            "messages": self.messages,
            "tools": self.tools,
            "tool_choice": self.tool_choice,
        });
        if let Some(effort) = &self.reasoning_effort {
            request["reasoning_effort"] = json!(effort);
        }
        request
    }

    /// Lower (never raise) the profile's default output budget to DSH's request
    /// cap. Provider identity, endpoint and auth references remain unchanged.
    pub fn profile_with_output_cap(&self, profile: &Value) -> Result<Value, String> {
        let fields = profile
            .as_object()
            .ok_or_else(|| "fixed provider profile must be an object".to_string())?;
        let Some(requested) = self.max_tokens else {
            return Ok(profile.clone());
        };
        let maximum = fields
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or_else(|| "profile max_output_tokens must be positive".to_string())?;
        if requested > maximum {
            return Err("DSH max_tokens exceeds the fixed profile output limit".into());
        }
        let configured = fields
            .get("default_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(maximum);
        let effective = configured.min(requested);
        let mut frozen = profile.clone();
        frozen["default_output_tokens"] = json!(effective);
        Ok(frozen)
    }
}

fn session_identity(headers: &HeaderMap) -> Result<String, String> {
    let values: Vec<String> = SESSION_HEADERS
        .iter()
        .filter_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
        .map(str::to_owned)
        .collect();
    let first = values
        .first()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "DSH session identity header is required".to_string())?;
    if values.iter().any(|value| value != first) || first.len() > 256 {
        return Err("DSH session identity headers must agree and be bounded".into());
    }
    Ok(first.clone())
}

fn validate_history_messages(messages: &[Value]) -> Result<(), String> {
    validate_messages(messages, true)
}

fn validate_messages(messages: &[Value], require_closed_tools: bool) -> Result<(), String> {
    let mut pending = HashMap::<String, String>::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for message in messages {
        let object = message
            .as_object()
            .ok_or_else(|| "each DSH message must be an object".to_string())?;
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "DSH message role is required".to_string())?;
        if !pending.is_empty() && role != "tool" {
            return Err("DSH tool calls must be closed by results before a later message".into());
        }
        match role {
            "system" | "developer" | "user" => {
                if object.keys().any(|key| key != "role" && key != "content")
                    || object.get("content").and_then(Value::as_str).is_none()
                {
                    return Err("DSH system/user messages must contain plain text only".into());
                }
            }
            "assistant" => {
                if object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "role"
                            | "content"
                            | "tool_calls"
                            | "refusal"
                            | "annotations"
                            | "reasoning_content"
                            | "reasoning"
                            | "reasoning_text"
                            | "reasoning_details"
                    )
                }) {
                    return Err("unsupported DSH assistant message field".into());
                }
                let calls = object.get("tool_calls");
                let content_valid = object
                    .get("content")
                    .is_some_and(|content| content.is_string() || content.is_null());
                if !content_valid && calls.is_none() {
                    return Err("assistant message requires text content or tool calls".into());
                }
                if let Some(calls) = calls {
                    let calls = calls
                        .as_array()
                        .filter(|calls| !calls.is_empty())
                        .ok_or_else(|| {
                            "assistant tool_calls must be a nonempty array".to_string()
                        })?;
                    for call in calls {
                        let call_obj = call
                            .as_object()
                            .ok_or_else(|| "native tool call must be an object".to_string())?;
                        if call_obj
                            .keys()
                            .any(|key| !matches!(key.as_str(), "id" | "type" | "function"))
                        {
                            return Err("native tool call contains unsupported fields".into());
                        }
                        let id = call_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.trim().is_empty())
                            .ok_or_else(|| "native tool call ID must be nonempty".to_string())?;
                        if !seen.insert(id.to_owned()) {
                            return Err(
                                "native tool call IDs must be unique in this history".into()
                            );
                        }
                        if call_obj.get("type").and_then(Value::as_str) != Some("function") {
                            return Err("only native function tool calls are supported".into());
                        }
                        let function = call_obj
                            .get("function")
                            .and_then(Value::as_object)
                            .ok_or_else(|| "native tool call requires function data".to_string())?;
                        if function
                            .keys()
                            .any(|key| !matches!(key.as_str(), "name" | "arguments"))
                        {
                            return Err("native tool function contains unsupported fields".into());
                        }
                        let name = function
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.trim().is_empty())
                            .ok_or_else(|| {
                                "native tool call function name is required".to_string()
                            })?;
                        let arguments = function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                "native tool call arguments must be a JSON string".to_string()
                            })?;
                        let parsed: Value = serde_json::from_str(arguments).map_err(|_| {
                            "native tool call arguments are incomplete or invalid JSON".to_string()
                        })?;
                        if !parsed.is_object() {
                            return Err("native tool call arguments must encode an object".into());
                        }
                        pending.insert(id.to_owned(), name.to_owned());
                    }
                }
            }
            "tool" => {
                if object.keys().any(|key| {
                    !matches!(key.as_str(), "role" | "content" | "tool_call_id" | "name")
                }) || object.get("content").and_then(Value::as_str).is_none()
                {
                    return Err("DSH tool results must contain supported text fields only".into());
                }
                let id = object
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| "tool result call ID is required".to_string())?;
                let expected_name = pending
                    .remove(id)
                    .ok_or_else(|| "tool result must match a prior native call ID".to_string())?;
                if object
                    .get("name")
                    .is_some_and(|name| name.as_str() != Some(expected_name.as_str()))
                {
                    return Err("tool result name does not match its native call".into());
                }
            }
            _ => return Err("unsupported DSH message role".into()),
        }
    }
    if require_closed_tools && !pending.is_empty() {
        return Err("DSH full history ends with unresolved native tool calls".into());
    }
    Ok(())
}

/// In-memory strict full-history cursor. Production hosts must persist it with
/// durable turn receipts and serialize requests per session; this helper alone
/// is not safe for restart/replay guarantees.
#[derive(Clone, Debug, Default)]
pub struct DshSessionHistory {
    sessions: HashMap<String, Vec<Value>>,
    generated_indices: HashMap<String, std::collections::HashSet<usize>>,
}

impl DshSessionHistory {
    /// Check that DSH sent the accepted history as an exact prefix and return
    /// only the new native messages. Generated assistant tool arguments allow
    /// structural JSON equivalence because PiAi parses and reserializes them.
    pub fn new_messages(&self, request: &DshChatRequest) -> Result<Vec<Value>, String> {
        let accepted = self
            .sessions
            .get(&request.session_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if request.messages.len() < accepted.len() {
            return Err("DSH history was truncated or rolled back".into());
        }
        let generated = self.generated_indices.get(&request.session_id);
        for (index, prior) in accepted.iter().enumerate() {
            let received = &request.messages[index];
            let matches = if generated.is_some_and(|indices| indices.contains(&index)) {
                generated_assistant_equivalent(prior, received)
            } else {
                prior == received
            };
            if !matches {
                return Err("DSH accepted history prefix changed".into());
            }
        }
        let additions = request.messages[accepted.len()..].to_vec();
        if additions.is_empty() {
            return Err("DSH request contains no new user or tool-result input".into());
        }
        Ok(additions)
    }

    /// Commit the exact full request history and the accepted assistant message
    /// only after the host has successfully delivered the turn.
    pub fn commit_reply(
        &mut self,
        request: &DshChatRequest,
        assistant_message: Value,
    ) -> Result<(), String> {
        let role = assistant_message.get("role").and_then(Value::as_str);
        if role != Some("assistant") {
            return Err("only an assistant message can be committed to DSH history".into());
        }
        validate_messages(&[assistant_message.clone()], false)?;
        // Require the current persisted cursor still to match the request's old
        // prefix, preventing accidental out-of-order commits in a serialized host.
        let current = self
            .sessions
            .get(&request.session_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if current.len() > request.messages.len()
            || current
                .iter()
                .zip(&request.messages)
                .any(|(left, right)| left != right)
        {
            return Err("DSH session advanced while this turn was in flight".into());
        }
        let generated_index = request.messages.len();
        let mut next = request.messages.clone();
        next.push(assistant_message);
        self.sessions.insert(request.session_id.clone(), next);
        self.generated_indices
            .entry(request.session_id.clone())
            .or_default()
            .insert(generated_index);
        Ok(())
    }
}

fn generated_assistant_equivalent(expected: &Value, actual: &Value) -> bool {
    if expected == actual {
        return true;
    }
    if expected.get("role").and_then(Value::as_str) != Some("assistant")
        || actual.get("role").and_then(Value::as_str) != Some("assistant")
    {
        return false;
    }
    for field in [
        "content",
        "refusal",
        "annotations",
        "reasoning_content",
        "reasoning",
        "reasoning_text",
        "reasoning_details",
    ] {
        if expected.get(field) != actual.get(field) {
            return false;
        }
    }
    let Some(expected_calls) = expected.get("tool_calls") else {
        return actual.get("tool_calls").is_none();
    };
    let Some(actual_calls) = actual.get("tool_calls").and_then(Value::as_array) else {
        return false;
    };
    let Some(expected_calls) = expected_calls.as_array() else {
        return false;
    };
    if expected_calls.len() != actual_calls.len() {
        return false;
    }
    expected_calls
        .iter()
        .zip(actual_calls)
        .all(|(left, right)| {
            left.get("id") == right.get("id")
                && left.get("type") == right.get("type")
                && left.get("function").and_then(|f| f.get("name"))
                    == right.get("function").and_then(|f| f.get("name"))
                && parse_arguments(left) == parse_arguments(right)
        })
}

fn parse_arguments(call: &Value) -> Option<Value> {
    let arguments = call.get("function")?.get("arguments")?.as_str()?;
    serde_json::from_str(arguments).ok()
}

/// Encode a complete native Chat Completion reply as the buffered SSE frames
/// expected by DSH PiAi. Tool calls receive stable array-order indices and their
/// original IDs/arguments are retained. Missing provider usage stays omitted.
pub fn buffered_sse(reply: &Value, include_usage: bool) -> Result<Vec<u8>, String> {
    let choices = reply
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| choices.len() == 1)
        .ok_or_else(|| "DSH reply requires exactly one Chat choice".to_string())?;
    let choice = &choices[0];
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "DSH reply requires an assistant message".to_string())?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err("DSH reply role must be assistant".into());
    }
    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .filter(|reason| matches!(*reason, "stop" | "tool_calls"))
        .ok_or_else(|| "DSH reply requires a supported finish_reason".to_string())?;
    let calls = message.get("tool_calls").and_then(Value::as_array);
    if finish == "tool_calls" && calls.is_none_or(Vec::is_empty) {
        return Err("tool_calls finish_reason requires complete calls".into());
    }
    if calls.is_some() && finish != "tool_calls" {
        return Err("assistant tool calls require tool_calls finish_reason".into());
    }
    if finish != "tool_calls" && message.get("content").and_then(Value::as_str).is_none() {
        return Err("text finish_reason requires complete assistant text".into());
    }
    validate_messages(&[Value::Object(message.clone())], false)?;
    let mut delta = Map::new();
    for field in [
        "role",
        "content",
        "tool_calls",
        "refusal",
        "annotations",
        "reasoning_content",
        "reasoning",
        "reasoning_text",
        "reasoning_details",
    ] {
        if let Some(value) = message.get(field) {
            delta.insert(field.to_owned(), value.clone());
        }
    }
    if let Some(calls) = calls {
        let indexed: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let mut call = call
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "tool call must be an object".to_string())?;
                if call
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                    || call.get("type").and_then(Value::as_str) != Some("function")
                    || call.get("function").and_then(Value::as_object).is_none()
                {
                    return Err("tool call identity/type/function is incomplete".to_string());
                }
                call.insert("index".into(), json!(index));
                Ok(Value::Object(call))
            })
            .collect::<Result<_, String>>()?;
        delta.insert("tool_calls".into(), Value::Array(indexed));
    }
    let base = json!({
        "id": reply.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_owned).unwrap_or_else(|| format!("sic-{}", Uuid::new_v4().simple())),
        "created": reply.get("created").and_then(Value::as_u64).unwrap_or_else(|| now_seconds()),
        "model": reply.get("model").cloned().unwrap_or(Value::Null),
        "system_fingerprint": reply.get("system_fingerprint").cloned().unwrap_or(Value::Null),
        "object":"chat.completion.chunk"
    });
    let mut first = base
        .as_object()
        .cloned()
        .ok_or_else(|| "SSE base must be an object".to_string())?;
    first.insert(
        "choices".into(),
        json!([{"index":0,"delta":Value::Object(delta),"finish_reason":Value::Null}]),
    );
    let mut terminal = base
        .as_object()
        .cloned()
        .ok_or_else(|| "SSE base must be an object".to_string())?;
    terminal.insert(
        "choices".into(),
        json!([{"index":0,"delta":{},"finish_reason":finish}]),
    );
    let mut output = String::new();
    for chunk in [Value::Object(first), Value::Object(terminal)] {
        output.push_str("data: ");
        output.push_str(
            &serde_json::to_string(&chunk)
                .map_err(|_| "could not encode DSH SSE chunk".to_string())?,
        );
        output.push_str("\n\n");
    }
    if include_usage {
        if let Some(usage) = reply.get("usage") {
            if !usage.is_null() {
                let mut chunk = base
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "SSE base must be an object".to_string())?;
                chunk.insert("choices".into(), json!([]));
                chunk.insert("usage".into(), usage.clone());
                output.push_str("data: ");
                output.push_str(
                    &serde_json::to_string(&Value::Object(chunk))
                        .map_err(|_| "could not encode DSH usage chunk".to_string())?,
                );
                output.push_str("\n\n");
            }
        }
    }
    output.push_str("data: [DONE]\n\n");
    Ok(output.into_bytes())
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
