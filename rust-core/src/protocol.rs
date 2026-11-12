//! Conservative, lossless ingress parsing for the SIC protocol boundary.
//!
//! The request sidecar is authoritative for roles, controls, tool calls, and
//! ordering. Only explicitly supported text fields become editable units.
//! Images remain immutable inline references in the sidecar.

use serde_json::{Map, Value, json};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
pub struct TextUnit {
    pub unit_id: String,
    pub text: String,
    /// JSON path segments (object keys or array indices) into `sidecar`.
    pub anchor: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedRequest {
    format: String,
    /// Exact accepted native request, retained as immutable protocol sidecar.
    sidecar: Value,
    pub units: Vec<TextUnit>,
}

impl DecodedRequest {
    /// Read the original request without granting mutation access.
    pub fn sidecar(&self) -> &Value {
        &self.sidecar
    }

    /// Name of the native wire format accepted by `decode`.
    pub fn format(&self) -> &str {
        &self.format
    }
}

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| err(format!("{label} must be an object")))
}

fn fields(value: &Value, allowed: &[&str], label: &str) -> Result<(), String> {
    let map = object(value, label)?;
    if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(err(format!("unsupported {label} field: {key}")));
    }
    Ok(())
}

fn required<'a>(value: &'a Value, key: &str, label: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| err(format!("{label} requires {key}")))
}

fn text<'a>(value: &'a Value, label: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| err(format!("{label} must be text")))
}

fn add_unit(units: &mut Vec<TextUnit>, value: &Value, path: Vec<Value>) -> Result<(), String> {
    let value = text(value, "editable text")?;
    let id = format!("u{}", units.len() + 1);
    units.push(TextUnit {
        unit_id: id,
        text: value.to_owned(),
        anchor: Value::Array(path),
    });
    Ok(())
}

fn path(base: &[Value], item: impl Into<Value>) -> Vec<Value> {
    let mut result = base.to_vec();
    result.push(item.into());
    result
}

fn inline_image_uri(uri: &str) -> Result<(), String> {
    let (header, payload) = uri
        .split_once(',')
        .ok_or_else(|| err("image must be an inline base64 data URI"))?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|h| h.strip_suffix(";base64"))
        .ok_or_else(|| err("remote or non-base64 image references are unsupported"))?;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return Err(err("unsupported inline image MIME type"));
    }
    if payload.is_empty()
        || payload
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='))
    {
        return Err(err("invalid base64 image payload"));
    }
    if payload.len() % 4 != 0
        || payload.matches('=').count() > 2
        || (payload.contains('=') && !payload.ends_with('='))
    {
        return Err(err("invalid base64 padding"));
    }
    Ok(())
}

fn validate_chat_tools(request: &Value) -> Result<(), String> {
    if let Some(tools) = request.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| err("Chat tools must be an array"))?;
        for tool in tools {
            fields(tool, &["type", "function"], "Chat tool")?;
            if required(tool, "type", "Chat tool")?.as_str() != Some("function") {
                return Err(err("only function tools are supported"));
            }
            let function = required(tool, "function", "Chat tool")?;
            fields(
                function,
                &["name", "description", "parameters", "strict"],
                "Chat function schema",
            )?;
            let name = text(
                required(function, "name", "Chat function schema")?,
                "function name",
            )?;
            if name.is_empty() {
                return Err(err("function name must not be empty"));
            }
            if function.get("description").is_some_and(|v| !v.is_string())
                || function.get("parameters").is_some_and(|v| !v.is_object())
                || function.get("strict").is_some_and(|v| !v.is_boolean())
            {
                return Err(err("invalid Chat function schema field"));
            }
        }
    }
    Ok(())
}

fn add_chat_content(
    units: &mut Vec<TextUnit>,
    content: &Value,
    base: Vec<Value>,
    role: &str,
) -> Result<(), String> {
    if content.is_null() {
        return Ok(());
    }
    if let Some(value) = content.as_str() {
        return add_unit(
            units,
            &Value::String(value.to_owned()),
            path(&base, "content"),
        );
    }
    let parts = content
        .as_array()
        .ok_or_else(|| err("Chat content must be text, supported blocks, or null"))?;
    for (index, part) in parts.iter().enumerate() {
        let kind = part
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| err("Chat content block requires a type"))?;
        match kind {
            "text" => {
                fields(part, &["type", "text"], "Chat text block")?;
                let text = required(part, "text", "Chat text block")?;
                add_unit(
                    units,
                    text,
                    path(&path(&path(&base, "content"), index), "text"),
                )?;
            }
            "image_url" => {
                fields(part, &["type", "image_url"], "Chat image block")?;
                let image = required(part, "image_url", "Chat image block")?;
                fields(image, &["url", "detail"], "Chat image reference")?;
                let uri = text(required(image, "url", "Chat image reference")?, "image URL")?;
                inline_image_uri(uri)?;
                if image
                    .get("detail")
                    .is_some_and(|d| !matches!(d.as_str(), Some("low" | "high" | "auto")))
                {
                    return Err(err("unsupported image detail"));
                }
                // The image and role remain untouched in the sidecar.
                let _ = role;
            }
            _ => return Err(err("unsupported Chat content block")),
        }
    }
    Ok(())
}

fn decode_chat(request: &Value) -> Result<Vec<TextUnit>, String> {
    fields(
        request,
        &[
            "messages",
            "model",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "temperature",
            "top_p",
            "max_tokens",
            "max_completion_tokens",
            "stop",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "response_format",
            "user",
            "store",
            "reasoning_effort",
            "prompt_cache_key",
            "prompt_cache_retention",
        ],
        "Chat request",
    )?;
    let messages = required(request, "messages", "Chat request")?
        .as_array()
        .ok_or_else(|| err("messages must be an array"))?;
    validate_chat_tools(request)?;
    let mut units = Vec::new();
    let mut calls = HashSet::<String>::new();
    let mut results = HashSet::<String>::new();
    for (mi, message) in messages.iter().enumerate() {
        fields(
            message,
            &[
                "role",
                "content",
                "tool_calls",
                "tool_call_id",
                "name",
                "reasoning_content",
            ],
            "Chat message",
        )?;
        let role = text(required(message, "role", "Chat message")?, "Chat role")?;
        if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
            return Err(err("unsupported Chat role"));
        }
        if let Some(reasoning) = message.get("reasoning_content") {
            if role != "assistant" || (!reasoning.is_null() && !reasoning.is_string()) {
                return Err(err("reasoning_content must be assistant text or null"));
            }
            // Preserve provider-native reasoning in the sidecar, not as an
            // independently editable text unit. Some Chat providers require
            // it unchanged while continuing a tool-calling turn.
        }
        if calls.difference(&results).next().is_some() && role != "tool" {
            return Err(err(
                "native tool results must close open calls before the next message",
            ));
        }
        if let Some(content) = message.get("content") {
            if matches!(role, "system" | "developer") {
                // Instructions are protocol/control context, not editable body.
                if !content.is_null() && !content.is_string() {
                    return Err(err(
                        "protected Chat instructions must be plain text or null",
                    ));
                }
            } else {
                add_chat_content(
                    &mut units,
                    content,
                    vec![json!("messages"), json!(mi)],
                    role,
                )?;
            }
        }
        if let Some(tool_calls) = message.get("tool_calls") {
            if role != "assistant" {
                return Err(err("only assistant messages can contain tool calls"));
            }
            let tool_calls = tool_calls
                .as_array()
                .ok_or_else(|| err("tool_calls must be an array"))?;
            for (call_index, call) in tool_calls.iter().enumerate() {
                fields(call, &["id", "type", "function", "index"], "Chat tool call")?;
                let id = text(required(call, "id", "Chat tool call")?, "tool call id")?;
                if id.is_empty() || !calls.insert(id.to_owned()) {
                    return Err(err("empty or duplicate tool call id"));
                }
                if call.get("type").and_then(Value::as_str) != Some("function") {
                    return Err(err("only function tool calls are supported"));
                }
                if call
                    .get("index")
                    .is_some_and(|v| v.as_u64() != Some(call_index as u64))
                {
                    return Err(err("unsupported Chat tool call index"));
                }
                let function = required(call, "function", "Chat tool call")?;
                fields(function, &["name", "arguments"], "Chat function call")?;
                if text(
                    required(function, "name", "Chat function call")?,
                    "function name",
                )?
                .is_empty()
                {
                    return Err(err("function name must not be empty"));
                }
                let _ = text(
                    required(function, "arguments", "Chat function call")?,
                    "function arguments",
                )?;
            }
        }
        if role == "tool" {
            let id = text(
                required(message, "tool_call_id", "Chat tool result")?,
                "tool_call_id",
            )?;
            if !calls.contains(id) || !results.insert(id.to_owned()) {
                return Err(err("orphan or duplicate Chat tool result"));
            }
        }
    }
    Ok(units)
}

fn decode_responses(request: &Value) -> Result<Vec<TextUnit>, String> {
    fields(
        request,
        &[
            "model",
            "input",
            "instructions",
            "tools",
            "temperature",
            "top_p",
            "max_output_tokens",
            "tool_choice",
            "parallel_tool_calls",
        ],
        "Responses request",
    )?;
    if let Some(instructions) = request.get("instructions") {
        text(instructions, "Responses instructions")?;
    }
    if let Some(tools) = request.get("tools") {
        let list = tools
            .as_array()
            .ok_or_else(|| err("Responses tools must be an array"))?;
        for tool in list {
            fields(
                tool,
                &["type", "name", "description", "parameters", "strict"],
                "Responses function tool",
            )?;
            if tool.get("type").and_then(Value::as_str) != Some("function")
                || text(
                    required(tool, "name", "Responses function tool")?,
                    "function name",
                )?
                .is_empty()
            {
                return Err(err("only named Responses function tools are supported"));
            }
            if tool.get("description").is_some_and(|v| !v.is_string())
                || tool.get("parameters").is_some_and(|v| !v.is_object())
                || tool.get("strict").is_some_and(|v| !v.is_boolean())
            {
                return Err(err("invalid Responses function tool"));
            }
        }
    }
    if let Some(choice) = request.get("tool_choice") {
        if !matches!(choice.as_str(), Some("auto" | "none" | "required")) {
            return Err(err(
                "only auto/none/required Responses tool_choice is supported",
            ));
        }
    }
    let mut units = Vec::new();
    let input = request
        .get("input")
        .ok_or_else(|| err("Responses request requires input"))?;
    if let Some(value) = input.as_str() {
        add_unit(
            &mut units,
            &Value::String(value.to_owned()),
            vec![json!("input")],
        )?;
        return Ok(units);
    }
    let items = input
        .as_array()
        .ok_or_else(|| err("Responses input must be text or an item array"))?;
    for (ii, item) in items.iter().enumerate() {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        match kind {
            "message" => {
                fields(item, &["type", "role", "content"], "Responses message")?;
                let role = text(
                    required(item, "role", "Responses message")?,
                    "Responses role",
                )?;
                if !matches!(role, "system" | "developer" | "user" | "assistant") {
                    return Err(err("unsupported Responses message role"));
                }
                let content = required(item, "content", "Responses message")?;
                let base = vec![json!("input"), json!(ii), json!("content")];
                if let Some(value) = content.as_str() {
                    if !matches!(role, "system" | "developer") {
                        add_unit(&mut units, &Value::String(value.to_owned()), base)?;
                    }
                } else {
                    let blocks = content
                        .as_array()
                        .ok_or_else(|| err("Responses message content must be text or blocks"))?;
                    for (bi, block) in blocks.iter().enumerate() {
                        match block.get("type").and_then(Value::as_str) {
                            Some("input_text" | "output_text") => {
                                fields(block, &["type", "text"], "Responses text block")?;
                                if !matches!(role, "system" | "developer") {
                                    add_unit(
                                        &mut units,
                                        required(block, "text", "Responses text block")?,
                                        path(&path(&base, bi), "text"),
                                    )?;
                                }
                            }
                            Some("input_image") if role == "user" => {
                                fields(
                                    block,
                                    &["type", "image_url", "detail"],
                                    "Responses image block",
                                )?;
                                inline_image_uri(text(
                                    required(block, "image_url", "Responses image block")?,
                                    "image URL",
                                )?)?;
                                if block.get("detail").is_some_and(|v| {
                                    !matches!(v.as_str(), Some("low" | "high" | "auto"))
                                }) {
                                    return Err(err("unsupported image detail"));
                                }
                            }
                            _ => return Err(err("unsupported Responses content block")),
                        }
                    }
                }
            }
            "function_call" => {
                fields(
                    item,
                    &["type", "call_id", "name", "arguments"],
                    "Responses function call",
                )?;
                if text(
                    required(item, "call_id", "Responses function call")?,
                    "call_id",
                )?
                .is_empty()
                    || text(
                        required(item, "name", "Responses function call")?,
                        "function name",
                    )?
                    .is_empty()
                {
                    return Err(err("Responses call id and function name must be nonempty"));
                }
                let _ = text(
                    required(item, "arguments", "Responses function call")?,
                    "function arguments",
                )?;
            }
            "function_call_output" => {
                fields(
                    item,
                    &["type", "call_id", "output"],
                    "Responses function output",
                )?;
                if text(
                    required(item, "call_id", "Responses function output")?,
                    "call_id",
                )?
                .is_empty()
                {
                    return Err(err("Responses output call_id must be nonempty"));
                }
                add_unit(
                    &mut units,
                    required(item, "output", "Responses function output")?,
                    vec![json!("input"), json!(ii), json!("output")],
                )?;
            }
            _ => return Err(err("unsupported Responses input item")),
        }
    }
    Ok(units)
}

fn add_anthropic_blocks(
    units: &mut Vec<TextUnit>,
    blocks: &[Value],
    base: Vec<Value>,
    role: &str,
) -> Result<(), String> {
    let mut saw_tool_use = false;
    let mut saw_text = false;
    for (bi, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                fields(block, &["type", "text"], "Anthropic text block")?;
                if saw_tool_use {
                    return Err(err("text after tool_use is rejected to avoid reordering"));
                }
                saw_text = true;
                add_unit(
                    units,
                    required(block, "text", "Anthropic text block")?,
                    path(&path(&base, bi), "text"),
                )?;
            }
            Some("image") => {
                fields(block, &["type", "source"], "Anthropic image block")?;
                let source = required(block, "source", "Anthropic image block")?;
                fields(
                    source,
                    &["type", "media_type", "data"],
                    "Anthropic image source",
                )?;
                if source.get("type").and_then(Value::as_str) != Some("base64") {
                    return Err(err("Anthropic images must be immutable base64 sources"));
                }
                let mime = text(
                    required(source, "media_type", "Anthropic image source")?,
                    "image MIME",
                )?;
                let data = text(
                    required(source, "data", "Anthropic image source")?,
                    "image data",
                )?;
                inline_image_uri(&format!("data:{mime};base64,{data}"))?;
                if role == "assistant" {
                    return Err(err("assistant image input is unsupported"));
                }
            }
            Some("tool_use") => {
                fields(
                    block,
                    &["type", "id", "name", "input"],
                    "Anthropic tool use",
                )?;
                if role != "assistant"
                    || text(required(block, "id", "Anthropic tool use")?, "tool use id")?.is_empty()
                    || text(required(block, "name", "Anthropic tool use")?, "tool name")?.is_empty()
                    || !required(block, "input", "Anthropic tool use")?.is_object()
                {
                    return Err(err("invalid Anthropic tool_use"));
                }
                saw_tool_use = true;
            }
            Some("tool_result") => {
                fields(
                    block,
                    &["type", "tool_use_id", "content", "is_error"],
                    "Anthropic tool result",
                )?;
                if role != "user"
                    || saw_text
                    || saw_tool_use
                    || text(
                        required(block, "tool_use_id", "Anthropic tool result")?,
                        "tool_use_id",
                    )?
                    .is_empty()
                {
                    return Err(err("Anthropic tool_result ordering/role unsupported"));
                }
                if let Some(is_error) = block.get("is_error") {
                    if !is_error.is_boolean() {
                        return Err(err("tool_result is_error must be boolean"));
                    }
                }
                let content = required(block, "content", "Anthropic tool result")?;
                if let Some(value) = content.as_str() {
                    add_unit(
                        units,
                        &Value::String(value.to_owned()),
                        path(&path(&base, bi), "content"),
                    )?;
                } else if let Some(parts) = content.as_array() {
                    for (pi, part) in parts.iter().enumerate() {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            fields(part, &["type", "text"], "Anthropic tool result text")?;
                            add_unit(
                                units,
                                required(part, "text", "Anthropic tool result text")?,
                                path(&path(&path(&path(&base, bi), "content"), pi), "text"),
                            )?;
                        } else if part.get("type").and_then(Value::as_str) == Some("image") {
                            validate_anthropic_image(part)?;
                        } else {
                            return Err(err("unsupported Anthropic tool result block"));
                        }
                    }
                } else {
                    return Err(err("Anthropic tool_result content must be text or blocks"));
                }
            }
            _ => return Err(err("unsupported Anthropic content block")),
        }
    }
    Ok(())
}

fn validate_anthropic_image(block: &Value) -> Result<(), String> {
    fields(block, &["type", "source"], "Anthropic image block")?;
    let source = required(block, "source", "Anthropic image block")?;
    fields(
        source,
        &["type", "media_type", "data"],
        "Anthropic image source",
    )?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return Err(err("Anthropic images must be immutable base64 sources"));
    }
    let mime = text(
        required(source, "media_type", "Anthropic image source")?,
        "image MIME",
    )?;
    let data = text(
        required(source, "data", "Anthropic image source")?,
        "image data",
    )?;
    inline_image_uri(&format!("data:{mime};base64,{data}"))
}

fn decode_anthropic(request: &Value) -> Result<Vec<TextUnit>, String> {
    fields(
        request,
        &[
            "model",
            "system",
            "messages",
            "tools",
            "temperature",
            "top_p",
            "max_tokens",
            "stop_sequences",
        ],
        "Anthropic request",
    )?;
    if let Some(system) = request.get("system") {
        if let Some(_) = system.as_str() { /* protected system text stays only in sidecar */
        } else if let Some(blocks) = system.as_array() {
            for block in blocks {
                fields(
                    block,
                    &["type", "text", "cache_control"],
                    "Anthropic system block",
                )?;
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(err("only Anthropic system text is supported"));
                }
                let _ = text(
                    required(block, "text", "Anthropic system block")?,
                    "system text",
                )?;
                if block.get("cache_control").is_some() {
                    return Err(err("system cache_control is not in the lossless subset"));
                }
            }
        } else {
            return Err(err("Anthropic system must be text or text blocks"));
        }
    }
    if let Some(tools) = request.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| err("Anthropic tools must be an array"))?;
        for tool in tools {
            fields(
                tool,
                &["name", "description", "input_schema"],
                "Anthropic tool schema",
            )?;
            if text(
                required(tool, "name", "Anthropic tool schema")?,
                "tool name",
            )?
            .is_empty()
                || !required(tool, "input_schema", "Anthropic tool schema")?.is_object()
            {
                return Err(err("invalid Anthropic tool schema"));
            }
            if tool.get("description").is_some_and(|v| !v.is_string()) {
                return Err(err("tool description must be text"));
            }
        }
    }
    let messages = required(request, "messages", "Anthropic request")?
        .as_array()
        .ok_or_else(|| err("Anthropic messages must be an array"))?;
    let mut units = Vec::new();
    for (mi, message) in messages.iter().enumerate() {
        fields(message, &["role", "content"], "Anthropic message")?;
        let role = text(
            required(message, "role", "Anthropic message")?,
            "Anthropic role",
        )?;
        if !matches!(role, "user" | "assistant") {
            return Err(err("Anthropic message role must be user or assistant"));
        }
        let content = required(message, "content", "Anthropic message")?;
        if let Some(value) = content.as_str() {
            add_unit(
                &mut units,
                &Value::String(value.to_owned()),
                vec![json!("messages"), json!(mi), json!("content")],
            )?;
        } else {
            let blocks = content
                .as_array()
                .filter(|xs| !xs.is_empty())
                .ok_or_else(|| err("Anthropic content must be text or nonempty blocks"))?;
            add_anthropic_blocks(
                &mut units,
                blocks,
                vec![json!("messages"), json!(mi), json!("content")],
                role,
            )?;
        }
    }
    Ok(units)
}

/// Decode one supported native request. Unknown controls and lossy content are rejected.
pub fn decode(format: &str, request: &Value) -> Result<DecodedRequest, String> {
    let units = match format {
        "openai-chat" => decode_chat(request)?,
        "openai-responses" => decode_responses(request)?,
        "anthropic-messages" => decode_anthropic(request)?,
        _ => return Err(err("unsupported protocol format")),
    };
    Ok(DecodedRequest {
        format: format.to_owned(),
        sidecar: request.clone(),
        units,
    })
}

/// Rebuild the original protocol request, replacing only explicitly anchored text.
pub fn encode(decoded: &DecodedRequest) -> Result<Value, String> {
    if !matches!(
        decoded.format.as_str(),
        "openai-chat" | "openai-responses" | "anthropic-messages"
    ) {
        return Err(err("unsupported protocol format"));
    }
    let canonical = decode(&decoded.format, &decoded.sidecar)?;
    if canonical.units.len() != decoded.units.len() {
        return Err(err("editable unit set does not match the source sidecar"));
    }
    for (expected, actual) in canonical.units.iter().zip(&decoded.units) {
        if expected.unit_id != actual.unit_id || expected.anchor != actual.anchor {
            return Err(err(
                "unit identity or anchor differs from the source sidecar",
            ));
        }
    }
    let mut result = decoded.sidecar.clone();
    let mut seen = HashSet::new();
    for unit in &decoded.units {
        if !seen.insert(unit.unit_id.as_str()) {
            return Err(err("duplicate unit_id"));
        }
        let path = unit
            .anchor
            .as_array()
            .ok_or_else(|| err("unit anchor must be a JSON path array"))?;
        if path.is_empty() {
            return Err(err("unit anchor must not be empty"));
        }
        let mut cursor = &mut result;
        for segment in &path[..path.len() - 1] {
            cursor = match segment {
                Value::String(key) => cursor
                    .get_mut(key)
                    .ok_or_else(|| err("unit anchor no longer resolves"))?,
                Value::Number(number) => cursor
                    .get_mut(number.as_u64().ok_or_else(|| err("invalid array anchor"))? as usize)
                    .ok_or_else(|| err("unit anchor no longer resolves"))?,
                _ => return Err(err("anchor segments must be object keys or array indices")),
            };
        }
        let leaf = path.last().unwrap();
        let target = match leaf {
            Value::String(key) => cursor
                .get_mut(key)
                .ok_or_else(|| err("unit anchor no longer resolves"))?,
            Value::Number(number) => cursor
                .get_mut(number.as_u64().ok_or_else(|| err("invalid array anchor"))? as usize)
                .ok_or_else(|| err("unit anchor no longer resolves"))?,
            _ => return Err(err("anchor segments must be object keys or array indices")),
        };
        if !target.is_string() {
            return Err(err("unit anchor no longer points to text"));
        }
        *target = Value::String(unit.text.clone());
    }
    Ok(result)
}
