//! Fixed OpenAI Chat-compatible HTTP provider path.
//!
//! The host supplies credentials through a resolver and supplies a frozen
//! complete-input token count. This module neither reads environment variables
//! nor infers tokenization. It deliberately supports normalized text Chat
//! messages with frozen function tools, one non-streaming choice, and one
//! complete response body per request.

use reqwest::{Client, StatusCode, Url, header};
use serde_json::{Value, json};
use std::{net::IpAddr, time::Duration};

const MAX_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const INVALID_CHOICE_COUNT: &str = "provider response must contain exactly one choice";

/// A complete input count measured by the host before this call is dispatched.
/// `counter_id` identifies the declared host-side counter and is never described
/// as provider-native or billed usage by this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenInputCount {
    pub tokens: u64,
    pub counter_id: String,
}

/// Provider-reported response details. Missing usage remains `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatCompletionResult {
    pub requested_model: String,
    pub returned_model: String,
    pub finish_reason: String,
    pub usage: Option<Value>,
    pub raw_response: Value,
    /// Complete call objects with IDs and arguments preserved. Any optional
    /// streaming-style indexes are normalized to complete-array positions;
    /// the unmodified provider response remains in `raw_response`/`raw_body`.
    pub tool_calls: Option<Value>,
    /// Original bounded response bytes, retained in memory for caller auditing.
    pub raw_body: Vec<u8>,
    pub admission: AdmissionRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallRole {
    Worker,
    Editor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionRecord {
    pub input_tokens: u64,
    pub input_counter_id: String,
    pub tokenizer_model: String,
    pub tokenizer_status: String,
    pub output_budget: u64,
    pub context_window: u64,
}

/// Validate one host-configured fixed OpenAI-compatible model profile.
/// Supports direct Python `roles.*` profile objects and named profiles after the
/// caller has resolved the mapping. This function never reads credential values.
pub fn validate_openai_profile(profile: &Value) -> Result<(), String> {
    let fields = profile
        .as_object()
        .ok_or_else(|| "provider profile must be an object".to_string())?;
    if fields.keys().any(|key| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "api_key" | "api_key_value" | "authorization" | "access_token" | "secret"
        )
    }) {
        return Err(
            "inline credentials are forbidden; use api_key_env with the host resolver".into(),
        );
    }

    let model = required_string(fields, "model")?;
    let (family, model_name) = model
        .split_once('/')
        .ok_or_else(|| "fixed model must include an explicit provider prefix".to_string())?;
    if family != "openai" || model_name.trim().is_empty() {
        return Err("only explicitly fixed OpenAI Chat-compatible models are supported".into());
    }
    let base = required_string(fields, "api_base")?;
    let endpoint = validate_endpoint(base)?;
    if let Some(pinned) = fields.get("credential_origin") {
        let value = pinned
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "credential_origin must be a fixed origin URL".to_string())?;
        let origin = validate_endpoint(value)
            .map_err(|_| "credential_origin must be a fixed origin URL".to_string())?;
        if origin.path() != "/" {
            return Err("credential_origin must not include a path".into());
        }
        if endpoint.origin() != origin.origin() {
            return Err("provider endpoint does not match credential_origin".into());
        }
    }
    required_string(fields, "api_key_env")?;
    required_string(fields, "tokenizer_model")?;
    if let Some(status) = fields.get("tokenizer_status") {
        if status.as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err("tokenizer_status must be a nonempty string when provided".into());
        }
    }

    let context = positive_integer(fields, "context_window")?;
    let max_input = positive_integer(fields, "max_input_tokens")?;
    let max_output = positive_integer(fields, "max_output_tokens")?;
    if max_input > context || max_output > context {
        return Err("input/output token limit exceeds context_window".into());
    }
    if let Some(value) = fields.get("default_output_tokens") {
        let default = positive_integer_value(value, "default_output_tokens")?;
        if default > max_output {
            return Err("default_output_tokens exceeds max_output_tokens".into());
        }
    }
    if let Some(value) = fields.get("max_supported_output_tokens") {
        let supported = positive_integer_value(value, "max_supported_output_tokens")?;
        if supported < max_output {
            return Err("max_output_tokens exceeds max_supported_output_tokens".into());
        }
    }
    if let Some(value) = fields.get("timeout_seconds") {
        let timeout = positive_integer_value(value, "timeout_seconds")?;
        if timeout > MAX_TIMEOUT_SECONDS {
            return Err("timeout_seconds exceeds the finite provider limit".into());
        }
    }
    if fields
        .get("retry_editor_http_5xx")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("retry_editor_http_5xx must be a boolean".into());
    }
    if fields
        .get("retry_worker_http_5xx")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("retry_worker_http_5xx must be a boolean".into());
    }
    // Shape the opt-in retry before any request is sent. The same helper feeds
    // the DSH bridge, so validation and execution cannot disagree.
    crate::model_policy::worker_retry_policy(profile)?;
    if let Some(expected) = fields.get("expected_returned_models") {
        let names = expected
            .as_array()
            .ok_or_else(|| "expected_returned_models must be an array".to_string())?;
        if names.is_empty()
            || names
                .iter()
                .any(|item| item.as_str().is_none_or(|name| name.trim().is_empty()))
        {
            return Err("expected_returned_models must list nonempty model identities".into());
        }
    }
    Ok(())
}

/// Make one fixed, non-streaming normalized Chat request.
///
/// `resolve_secret` receives only the host reference name from `api_key_env`.
/// Resolver errors are replaced with a generic message so secret material is
/// never copied into returned errors. There are no redirects or retries.
pub async fn chat_completion<F>(
    profile: &Value,
    request: &Value,
    call_role: CallRole,
    frozen_input: &FrozenInputCount,
    resolve_secret: F,
) -> Result<ChatCompletionResult, String>
where
    F: FnOnce(&str) -> Result<String, String>,
{
    chat_completion_observed(
        profile,
        request,
        call_role,
        frozen_input,
        resolve_secret,
        |_| {},
    )
    .await
}

/// The host observer receives the bounded raw HTTP body before JSON/tool
/// validation, so even a rejected model output can be archived. It must never
/// log credentials or treat the observed body as an accepted response.
pub async fn chat_completion_observed<F, O>(
    profile: &Value,
    request: &Value,
    call_role: CallRole,
    frozen_input: &FrozenInputCount,
    resolve_secret: F,
    observe: O,
) -> Result<ChatCompletionResult, String>
where
    F: FnOnce(&str) -> Result<String, String>,
    O: FnOnce(&[u8]),
{
    chat_completion_observed_status(
        profile,
        request,
        call_role,
        frozen_input,
        resolve_secret,
        observe,
        |_| {},
    )
    .await
}

/// Additionally observe an actual HTTP response status, if one arrives.
/// Timeouts and transport failures do not invoke this callback. This is an
/// observation only: the caller remains responsible for any retry policy.
pub async fn chat_completion_observed_status<F, O, S>(
    profile: &Value,
    request: &Value,
    call_role: CallRole,
    frozen_input: &FrozenInputCount,
    resolve_secret: F,
    observe: O,
    observe_status: S,
) -> Result<ChatCompletionResult, String>
where
    F: FnOnce(&str) -> Result<String, String>,
    O: FnOnce(&[u8]),
    S: FnOnce(StatusCode),
{
    validate_openai_profile(profile)?;
    if frozen_input.counter_id.trim().is_empty() {
        return Err("host input counter identifier is required".into());
    }
    let request_fields = request
        .as_object()
        .ok_or_else(|| "normalized Chat request must be an object".to_string())?;
    if request_fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "messages" | "tools" | "tool_choice" | "reasoning_effort"
        )
    }) {
        return Err(
            "unsupported Chat control field; fixed host profile controls cannot be overridden"
                .into(),
        );
    }
    let messages = request_fields
        .get("messages")
        .ok_or_else(|| "normalized Chat request requires messages".to_string())?;
    let tools = request_fields.get("tools").ok_or_else(|| {
        "normalized Chat request requires the frozen host tools array".to_string()
    })?;
    let tool_names = validate_tools(tools)?;
    let tool_choice = match request_fields.get("tool_choice") {
        None => "auto",
        Some(value) => value
            .as_str()
            .ok_or_else(|| "tool_choice must be the string auto".to_string())?,
    };
    if tool_choice != "auto" {
        return Err("tool_choice is fixed to auto".into());
    }
    validate_messages(messages, &tool_names)?;
    let reasoning_effort = request_fields
        .get("reasoning_effort")
        .and_then(Value::as_str);
    if request_fields.contains_key("reasoning_effort") && reasoning_effort.is_none() {
        return Err("reasoning_effort must be a string".into());
    }
    if let Some(effort) = reasoning_effort {
        if !crate::model_policy::REASONING_EFFORTS.contains(&effort) {
            return Err("unsupported reasoning_effort".into());
        }
    }

    let fields = profile.as_object().expect("profile validated as object");
    let max_input = positive_integer(fields, "max_input_tokens")?;
    let max_output = positive_integer(fields, "max_output_tokens")?;
    let context = positive_integer(fields, "context_window")?;
    let output = fields
        .get("default_output_tokens")
        .map(|value| positive_integer_value(value, "default_output_tokens"))
        .transpose()?
        .unwrap_or(max_output);

    if frozen_input.tokens > max_input {
        return Err("complete host-counted input exceeds max_input_tokens".into());
    }
    if output > max_output {
        return Err("selected output budget exceeds max_output_tokens".into());
    }
    let combined = frozen_input
        .tokens
        .checked_add(output)
        .ok_or_else(|| "combined input/output budget overflow".to_string())?;
    if combined > context {
        return Err("complete host-counted input plus output budget exceeds context_window".into());
    }

    let requested_model = required_string(fields, "model")?.to_string();
    let provider_model = requested_model
        .split_once('/')
        .map(|(_, model)| model)
        .ok_or_else(|| "fixed model must include an explicit provider prefix".to_string())?;
    let expected_models = expected_models(fields, &requested_model)?;
    let tokenizer_model = required_string(fields, "tokenizer_model")?.to_string();
    let tokenizer_status = fields
        .get("tokenizer_status")
        .and_then(Value::as_str)
        .unwrap_or("host-selected-not-provider-attested")
        .to_string();
    let api_key_env = required_string(fields, "api_key_env")?;
    let api_key = resolve_secret(api_key_env)
        .map_err(|_| "host credential resolver did not provide the configured key".to_string())?;
    if api_key.is_empty() {
        return Err("host credential resolver returned an empty key".into());
    }
    let endpoint = chat_endpoint(required_string(fields, "api_base")?)?;
    let timeout_seconds = fields
        .get("timeout_seconds")
        .map(|value| positive_integer_value(value, "timeout_seconds"))
        .transpose()?
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err("timeout_seconds exceeds the finite provider limit".into());
    }

    let mut body = json!({
        "model": provider_model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "stream": false,
        "n": 1,
        "max_tokens": output
    });
    if let Some(effort) = reasoning_effort {
        body["reasoning_effort"] = Value::String(effort.to_string());
    }

    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(timeout_seconds))
        .connect_timeout(Duration::from_secs(timeout_seconds.min(30)))
        .build()
        .map_err(|_| "could not construct fixed provider transport".to_string())?;
    let mut response = client
        .post(endpoint)
        .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "fixed provider request timed out".to_string()
            } else {
                "fixed provider transport failed".to_string()
            }
        })?;

    let status = response.status();
    observe_status(status);
    if status.is_redirection() {
        return Err("provider redirect rejected; redirects are disabled".into());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err("provider returned HTTP 429; request was not retried".into());
    }
    if !status.is_success() {
        return Err(format!("provider returned HTTP {}", status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("provider response exceeds the bounded response size".into());
    }
    let mut raw_body = Vec::new();
    while let Some(part) = response
        .chunk()
        .await
        .map_err(|_| "provider response read failed".to_string())?
    {
        if raw_body.len().saturating_add(part.len()) > MAX_RESPONSE_BYTES {
            return Err("provider response exceeds the bounded response size".into());
        }
        raw_body.extend_from_slice(&part);
    }
    observe(&raw_body);
    let raw_response: Value = serde_json::from_slice(&raw_body)
        .map_err(|_| "provider returned an incomplete or invalid JSON response".to_string())?;
    let returned_model = raw_response
        .get("model")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "provider response omitted returned model identity".to_string())?
        .to_string();
    if !expected_models
        .iter()
        .any(|expected| expected == &returned_model)
    {
        return Err(
            "provider returned a model identity outside the fixed profile allowlist".into(),
        );
    }

    let choices = raw_response
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| "provider response omitted choices".to_string())?;
    if choices.len() != 1 {
        return Err(INVALID_CHOICE_COUNT.into());
    }
    let choice = &choices[0];
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider response omitted completion status".to_string())?;
    if finish_reason == "length" {
        return Err("provider response was truncated at the output limit".into());
    }
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "provider response omitted assistant message".to_string())?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err("provider response role must be assistant".into());
    }
    if message
        .get("reasoning_content")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err("provider reasoning_content must be text or null".into());
    }
    let response_tool_calls = message.get("tool_calls");
    let tool_calls = match finish_reason {
        "stop" => {
            if response_tool_calls.is_some() {
                return Err("assistant tool calls require tool_calls completion status".into());
            }
            if message.get("content").and_then(Value::as_str).is_none() {
                return Err("provider response is not a complete text assistant message".into());
            }
            if call_role == CallRole::Editor {
                return Err("Editor delivery must use sic_edit_blocks".into());
            }
            None
        }
        "tool_calls" => {
            let calls = response_tool_calls
                .and_then(Value::as_array)
                .filter(|calls| !calls.is_empty())
                .ok_or_else(|| {
                    "tool_calls completion status requires complete tool calls".to_string()
                })?;
            // In a complete non-streaming response, array order and unique
            // call IDs define the calls. Some Chat-compatible providers add
            // a streaming `index` field and repeat zero for every call.
            // Normalize only that optional metadata; never rewrite IDs,
            // names, arguments or the archived raw response.
            let mut normalized = calls.clone();
            for (position, call) in normalized.iter_mut().enumerate() {
                let object = call
                    .as_object_mut()
                    .ok_or_else(|| "tool call must be an object".to_string())?;
                if let Some(index) = object.get_mut("index") {
                    if index.as_u64().is_none() {
                        return Err("native tool call index must be a nonnegative integer".into());
                    }
                    *index = json!(position);
                }
            }
            let normalized = Value::Array(normalized);
            let called = validate_tool_calls(&normalized, &tool_names)?;
            if call_role == CallRole::Editor
                && !called.iter().any(|(_, name)| name == "sic_edit_blocks")
            {
                return Err("Editor delivery must include sic_edit_blocks".into());
            }
            Some(normalized)
        }
        _ => return Err("provider returned an unsupported or incomplete completion status".into()),
    };

    let usage = raw_response.get("usage").cloned();
    Ok(ChatCompletionResult {
        requested_model,
        returned_model,
        finish_reason: finish_reason.to_string(),
        usage,
        raw_response,
        tool_calls,
        raw_body,
        admission: AdmissionRecord {
            input_tokens: frozen_input.tokens,
            input_counter_id: frozen_input.counter_id.clone(),
            tokenizer_model,
            tokenizer_status,
            output_budget: output,
            context_window: context,
        },
    })
}

fn validate_tools(tools: &Value) -> Result<Vec<String>, String> {
    let entries = tools
        .as_array()
        .ok_or_else(|| "tools must be a frozen host tools array".to_string())?;
    let mut names = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry
            .as_object()
            .ok_or_else(|| "each tool schema must be an object".to_string())?;
        if object.keys().any(|key| key != "type" && key != "function")
            || object.get("type").and_then(Value::as_str) != Some("function")
        {
            return Err("only OpenAI function tool schemas are supported".into());
        }
        let function = object
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| "tool schema requires a function object".to_string())?;
        if function.keys().any(|key| {
            !matches!(
                key.as_str(),
                "name" | "description" | "parameters" | "strict"
            )
        }) {
            return Err("unsupported tool schema control".into());
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| "tool function name must be nonempty".to_string())?;
        if names.iter().any(|known| known == name) {
            return Err("tool function names must be unique".into());
        }
        if !function.get("parameters").is_some_and(Value::is_object) {
            return Err("tool function parameters must be a JSON schema object".into());
        }
        names.push(name.to_string());
    }
    if !names.iter().any(|name| name == "sic_edit_blocks") {
        return Err("frozen common tools must include sic_edit_blocks".into());
    }
    Ok(names)
}

fn validate_messages(messages: &Value, tool_names: &[String]) -> Result<(), String> {
    let entries = messages
        .as_array()
        .ok_or_else(|| "messages must be a normalized array".to_string())?;
    if entries.is_empty() {
        return Err("messages must not be empty".into());
    }
    let mut pending = std::collections::BTreeMap::<String, String>::new();
    let mut seen_call_ids = std::collections::BTreeSet::<String>::new();
    for entry in entries {
        let object = entry
            .as_object()
            .ok_or_else(|| "each message must be an object".to_string())?;
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "message role must be a string".to_string())?;
        if !pending.is_empty() && role != "tool" {
            return Err(
                "every assistant tool call must be closed by its tool result before continuing"
                    .into(),
            );
        }
        match role {
            "system" | "developer" | "user" => {
                if object.keys().any(|key| key != "role" && key != "content")
                    || object.get("content").and_then(Value::as_str).is_none()
                {
                    return Err(
                        "system/developer/user messages must contain only plain-text role/content"
                            .into(),
                    );
                }
            }
            "assistant" => {
                if object.keys().any(|key| {
                    key != "role"
                        && key != "content"
                        && key != "tool_calls"
                        && key != "reasoning_content"
                }) {
                    return Err("unsupported assistant message field".into());
                }
                if object
                    .get("reasoning_content")
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                {
                    return Err("assistant reasoning_content must be text or null".into());
                }
                let content_valid = object
                    .get("content")
                    .is_some_and(|content| content.is_string() || content.is_null());
                let calls = object.get("tool_calls");
                if !content_valid && calls.is_none() {
                    return Err("assistant message requires text content or tool_calls".into());
                }
                if let Some(calls) = calls {
                    let calls = calls
                        .as_array()
                        .filter(|calls| !calls.is_empty())
                        .ok_or_else(|| {
                            "assistant tool_calls must be a nonempty array".to_string()
                        })?;
                    for (call_id, name) in
                        validate_tool_calls(&Value::Array(calls.clone()), tool_names)?
                    {
                        if !seen_call_ids.insert(call_id.clone()) {
                            return Err("assistant tool call IDs must be unique".into());
                        }
                        pending.insert(call_id, name);
                    }
                }
            }
            "tool" => {
                if object.keys().any(|key| {
                    key != "role" && key != "tool_call_id" && key != "content" && key != "name"
                }) {
                    return Err("unsupported tool result field".into());
                }
                if object.get("content").and_then(Value::as_str).is_none() {
                    return Err("tool result content must be plain text".into());
                }
                let call_id = object
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| "tool result requires a nonempty tool_call_id".to_string())?;
                let expected_name = pending.remove(call_id).ok_or_else(|| {
                    "tool result ID must match a prior assistant call".to_string()
                })?;
                if let Some(name) = object.get("name") {
                    if name.as_str() != Some(expected_name.as_str()) {
                        return Err("tool result name does not match its assistant call".into());
                    }
                }
            }
            _ => return Err("unsupported Chat message role".into()),
        }
    }
    if !pending.is_empty() {
        return Err("normalized request contains unresolved assistant tool calls".into());
    }
    Ok(())
}

/// Validate complete native tool calls while preserving their original Value.
/// The returned pairs are exact call ID/function name identities.
fn validate_tool_calls(
    calls: &Value,
    allowed_names: &[String],
) -> Result<Vec<(String, String)>, String> {
    let calls = calls
        .as_array()
        .ok_or_else(|| "tool_calls must be an array".to_string())?;
    let mut identities = Vec::with_capacity(calls.len());
    let mut ids = std::collections::BTreeSet::new();
    for (index, call) in calls.iter().enumerate() {
        let object = call
            .as_object()
            .ok_or_else(|| "tool call must be an object".to_string())?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "id" | "type" | "function" | "index"))
        {
            return Err("unsupported native tool call field".into());
        }
        if object
            .get("index")
            .is_some_and(|value| value.as_u64() != Some(index as u64))
        {
            return Err("native tool call index does not match its position".into());
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "tool call ID must be nonempty".to_string())?;
        if !ids.insert(id.to_string()) {
            return Err("tool call IDs must be unique".into());
        }
        if object.get("type").and_then(Value::as_str) != Some("function") {
            return Err("tool call type must be function".into());
        }
        let function = object
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| "tool call requires a function object".to_string())?;
        if function
            .keys()
            .any(|key| key != "name" && key != "arguments")
        {
            return Err("unsupported tool function call field".into());
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| allowed_names.iter().any(|allowed| allowed == name))
            .ok_or_else(|| "tool call name is not in the frozen host tool table".to_string())?;
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool call arguments must be a complete JSON string".to_string())?;
        let parsed: Value = serde_json::from_str(arguments)
            .map_err(|_| "tool call arguments are incomplete or invalid JSON".to_string())?;
        if !parsed.is_object() {
            return Err("tool call arguments must encode a JSON object".into());
        }
        identities.push((id.to_string(), name.to_string()));
    }
    Ok(identities)
}

fn validate_endpoint(base: &str) -> Result<Url, String> {
    let url = Url::parse(base).map_err(|_| "api_base is not a valid fixed URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("provider URL must omit credentials, query, and fragment".into());
    }
    let host = url.host_str().unwrap_or_default();
    let local = host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !local {
        return Err(
            "remote provider routes require HTTPS; HTTP is allowed only for loopback".into(),
        );
    }
    Ok(url)
}

fn chat_endpoint(base: &str) -> Result<Url, String> {
    let mut url = validate_endpoint(base)?;
    let path = url.path().trim_end_matches('/');
    let path = if path.is_empty() { "" } else { path };
    url.set_path(&format!("{path}/chat/completions"));
    Ok(url)
}

fn expected_models(
    fields: &serde_json::Map<String, Value>,
    requested_model: &str,
) -> Result<Vec<String>, String> {
    if let Some(expected) = fields.get("expected_returned_models") {
        return expected
            .as_array()
            .ok_or_else(|| "expected_returned_models must be an array".to_string())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "expected_returned_models must contain strings".to_string())
            })
            .collect();
    }
    let short = requested_model.split_once('/').map(|(_, name)| name);
    Ok(std::iter::once(requested_model.to_string())
        .chain(short.map(str::to_string))
        .collect())
}

fn required_string<'a>(
    fields: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str, String> {
    fields
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} must be a nonempty string"))
}

fn positive_integer(fields: &serde_json::Map<String, Value>, key: &str) -> Result<u64, String> {
    fields
        .get(key)
        .ok_or_else(|| format!("{key} is required"))
        .and_then(|value| positive_integer_value(value, key))
}

fn positive_integer_value(value: &Value, key: &str) -> Result<u64, String> {
    match value.as_u64() {
        Some(number) if number > 0 => Ok(number),
        _ => Err(format!("{key} must be a positive integer")),
    }
}
