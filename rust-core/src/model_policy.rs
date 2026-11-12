//! Host-allowlisted model profile validation and explicit Editor profile selection.
//!
//! This module validates configuration only. It never resolves credential values,
//! calls a provider, or counts tokens. Callers must count the complete rendered
//! request with their declared tokenizer and enforce the combined input/output
//! context budget before dispatch. `tokenizer_model` is a host-selected counter
//! identifier; it is not a claim that a provider-native tokenizer was used.

use serde_json::Value;

/// Accepted wire values are passed through unchanged. The configured Provider
/// decides which values or aliases it actually supports and how they map.
pub const REASONING_EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "ultra", "max",
];

/// Worker HTTP-5xx retry policy shared by profile validation and the DSH bridge.
///
/// `max_attempts` counts the first try, so the bridge retries at most
/// `max_attempts - 1` times. The defaults reproduce the pre-existing single
/// retry with a fixed 250 ms wait: an omitted key changes no behavior. The
/// opt-in switch stays `retry_worker_http_5xx`; these keys only shape it.
pub const DEFAULT_WORKER_RETRY_ATTEMPTS: u32 = 2;
pub const MAX_WORKER_RETRY_ATTEMPTS: u32 = 5;
pub const DEFAULT_WORKER_RETRY_BACKOFF_MS: u64 = 250;
pub const MAX_WORKER_RETRY_BACKOFF_MS: u64 = 5_000;

/// Validate and return `(max_attempts, backoff_ms)` from one Worker profile.
///
/// A malformed or out-of-range value is a configuration error, never a silent
/// fallback. An omitted value keeps the compatibility default.
pub fn worker_retry_policy(profile: &Value) -> Result<(u32, u64), String> {
    let fields = profile
        .as_object()
        .ok_or_else(|| "profile must be an object".to_string())?;
    let attempts = match fields.get("worker_retry_max_attempts") {
        None => DEFAULT_WORKER_RETRY_ATTEMPTS,
        Some(value) => value
            .as_u64()
            .filter(|attempts| (1..=MAX_WORKER_RETRY_ATTEMPTS as u64).contains(attempts))
            .map(|attempts| attempts as u32)
            .ok_or_else(|| "worker_retry_max_attempts must be an integer in 1..=5".to_string())?,
    };
    let backoff = match fields.get("worker_retry_backoff_ms") {
        None => DEFAULT_WORKER_RETRY_BACKOFF_MS,
        Some(value) => value
            .as_u64()
            .filter(|delay| *delay <= MAX_WORKER_RETRY_BACKOFF_MS)
            .ok_or_else(|| "worker_retry_backoff_ms must be an integer in 0..=5000".to_string())?,
    };
    Ok((attempts, backoff))
}

/// Validate the explicit host profile registry, role defaults, and profile maps.
///
/// Expected shape:
/// - `profiles`: optional profile name -> complete profile object registry
/// - `roles.worker` and `roles.editor`: either a complete inline profile, a
///   profile name, or `{ "profile": name }`
/// - optional `editor_profiles_by_level`: decimal level -> profile name
/// - optional `editor_profiles`: editor name -> profile name
///
/// Profiles require `model`, `tokenizer_model`, `max_input_tokens`,
/// `max_output_tokens`, and `context_window`. Optional output limits follow the
/// Python host configuration semantics. No 128K ceiling is imposed.
pub fn validate_config(config: &Value) -> Result<(), String> {
    let root = config
        .as_object()
        .ok_or_else(|| "host config must be a JSON object".to_string())?;
    let empty_profiles = serde_json::Map::new();
    let profiles = match root.get("profiles") {
        None => &empty_profiles,
        Some(value) => value
            .as_object()
            .ok_or_else(|| "profiles must be an object when provided".to_string())?,
    };

    for (name, profile) in profiles {
        if name.trim().is_empty() {
            return Err("profile name must not be empty".into());
        }
        validate_profile(profile).map_err(|e| format!("invalid profile: {e}"))?;
    }

    let roles = root
        .get("roles")
        .and_then(Value::as_object)
        .ok_or_else(|| "host config requires a roles object".to_string())?;
    for role in ["worker", "editor"] {
        let profile = role_profile_value(
            roles
                .get(role)
                .ok_or_else(|| format!("host config requires roles.{role}"))?,
            profiles,
        )?;
        validate_profile(profile).map_err(|e| format!("invalid roles.{role} profile: {e}"))?;
    }

    if let Some(level_map) = root.get("editor_profiles_by_level") {
        let level_map = level_map
            .as_object()
            .ok_or_else(|| "editor_profiles_by_level must be an object".to_string())?;
        for (level, profile_name) in level_map {
            let parsed = level.parse::<u32>().map_err(|_| {
                "editor profile level keys must be positive decimal integers".to_string()
            })?;
            if parsed == 0 || parsed.to_string() != *level {
                return Err(
                    "editor profile level keys must be canonical positive decimal integers".into(),
                );
            }
            let profile_name = profile_name.as_str().ok_or_else(|| {
                "level profile mappings must name a registered profile".to_string()
            })?;
            require_profile(profiles, profile_name)?;
        }
    }

    if let Some(editor_map) = root.get("editor_profiles") {
        let editor_map = editor_map
            .as_object()
            .ok_or_else(|| "editor_profiles must be an object".to_string())?;
        for (editor, profile_name) in editor_map {
            if editor.trim().is_empty() {
                return Err("editor mapping names must not be empty".into());
            }
            let profile_name = profile_name.as_str().ok_or_else(|| {
                "editor profile mappings must name a registered profile".to_string()
            })?;
            require_profile(profiles, profile_name)?;
        }
    }

    Ok(())
}

/// Resolve and deep-copy an Editor profile for a specific level and editor.
///
/// Selection precedence is explicit level mapping, explicit editor mapping,
/// then the host's default `roles.editor` profile. An invalid explicit mapping
/// is an error; it never silently switches to another model.
pub fn resolve_editor_profile(config: &Value, level: u32, editor: &str) -> Result<Value, String> {
    if level == 0 {
        return Err("Editor level must be greater than zero".into());
    }
    if editor.trim().is_empty() {
        return Err("Editor name must not be empty".into());
    }
    validate_config(config)?;
    let root = config
        .as_object()
        .ok_or_else(|| "host config must be a JSON object".to_string())?;
    let empty_profiles = serde_json::Map::new();
    let profiles = match root.get("profiles") {
        None => &empty_profiles,
        Some(value) => value
            .as_object()
            .ok_or_else(|| "profiles must be an object when provided".to_string())?,
    };

    let level_key = level.to_string();
    if let Some(profile_name) = root
        .get("editor_profiles_by_level")
        .and_then(Value::as_object)
        .and_then(|map| map.get(&level_key))
        .and_then(Value::as_str)
    {
        return cloned_profile(profiles, profile_name);
    }
    if let Some(profile_name) = root
        .get("editor_profiles")
        .and_then(Value::as_object)
        .and_then(|map| map.get(editor))
        .and_then(Value::as_str)
    {
        return cloned_profile(profiles, profile_name);
    }

    let roles = root
        .get("roles")
        .and_then(Value::as_object)
        .ok_or_else(|| "host config requires a roles object".to_string())?;
    let default_profile = role_profile_value(
        roles
            .get("editor")
            .ok_or_else(|| "host config requires roles.editor".to_string())?,
        profiles,
    )?;
    Ok(default_profile.clone())
}

/// Resolve the configured Worker profile using the same registry rules, without
/// allowing an Editor-level override to select a different Worker model.
pub fn resolve_worker_profile(config: &Value) -> Result<Value, String> {
    validate_config(config)?;
    let root = config.as_object().ok_or("host config must be an object")?;
    let empty = serde_json::Map::new();
    let profiles = root
        .get("profiles")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let worker = root
        .get("roles")
        .and_then(|v| v.get("worker"))
        .ok_or("roles.worker required")?;
    Ok(role_profile_value(worker, profiles)?.clone())
}

fn validate_profile(profile: &Value) -> Result<(), String> {
    let fields = profile
        .as_object()
        .ok_or_else(|| "profile must be an object".to_string())?;

    // Profiles may carry environment-variable names, never credential values.
    // Check key names only; do not inspect or resolve any secret-like value.
    if fields.keys().any(|key| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "api_key" | "api_key_value" | "authorization" | "access_token" | "secret"
        )
    }) {
        return Err("inline credentials are not allowed; configure a host-side reference".into());
    }

    required_nonempty_string(fields, "model")?;
    required_nonempty_string(fields, "tokenizer_model")?;
    if let Some(value) = fields.get("reasoning_effort") {
        if value
            .as_str()
            .is_none_or(|effort| !REASONING_EFFORTS.contains(&effort))
        {
            return Err("unsupported host reasoning_effort".into());
        }
    }
    let context = positive_integer(fields, "context_window")?;
    let max_input = positive_integer(fields, "max_input_tokens")?;
    let max_output = positive_integer(fields, "max_output_tokens")?;
    if max_input > context || max_output > context {
        return Err("input/output budget exceeds context_window".into());
    }

    if let Some(default) = fields.get("default_output_tokens") {
        let default = positive_integer_value(default, "default_output_tokens")?;
        if default > max_output {
            return Err("default_output_tokens exceeds max_output_tokens".into());
        }
    }
    if let Some(supported) = fields.get("max_supported_output_tokens") {
        let supported = positive_integer_value(supported, "max_supported_output_tokens")?;
        if supported < max_output {
            return Err("max_output_tokens exceeds max_supported_output_tokens".into());
        }
    }
    if let Some(status) = fields.get("tokenizer_status") {
        if status.as_str().is_none_or(|value| value.trim().is_empty()) {
            return Err("tokenizer_status must be a nonempty string when provided".into());
        }
    }
    // This is a host assertion used to distinguish plans, not a provider cache
    // key and never evidence of a physical hit. Omission remains unknown.
    if let Some(namespace) = fields.get("cache_namespace") {
        if namespace.as_str().is_none_or(|value| {
            value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
        }) {
            return Err("cache_namespace must be a nonempty, bounded host identifier".into());
        }
    }
    Ok(())
}

fn required_nonempty_string(
    fields: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<(), String> {
    match fields.get(key).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(format!("{key} must be a nonempty string")),
    }
}

fn positive_integer(fields: &serde_json::Map<String, Value>, key: &str) -> Result<u64, String> {
    let value = fields
        .get(key)
        .ok_or_else(|| format!("{key} is required"))?;
    positive_integer_value(value, key)
}

fn positive_integer_value(value: &Value, key: &str) -> Result<u64, String> {
    match value.as_u64() {
        Some(value) if value > 0 => Ok(value),
        _ => Err(format!("{key} must be a positive integer")),
    }
}

fn role_profile_value<'a>(
    role: &'a Value,
    profiles: &'a serde_json::Map<String, Value>,
) -> Result<&'a Value, String> {
    match role {
        Value::String(name) => {
            if name.trim().is_empty() {
                return Err("role profile name must not be empty".into());
            }
            require_profile(profiles, name)
        }
        Value::Object(object) if object.contains_key("profile") => {
            let name = object
                .get("profile")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "role profile reference must be a registered profile name".to_string()
                })?;
            if name.trim().is_empty() {
                return Err("role profile name must not be empty".into());
            }
            require_profile(profiles, name)
        }
        Value::Object(object) if object.contains_key("model") => Ok(role),
        _ => Err("role must be a complete profile or explicitly name a registered profile".into()),
    }
}

fn require_profile<'a>(
    profiles: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a Value, String> {
    profiles
        .get(name)
        .ok_or_else(|| "profile mapping refers to an unknown profile".to_string())
}

fn cloned_profile(profiles: &serde_json::Map<String, Value>, name: &str) -> Result<Value, String> {
    Ok(require_profile(profiles, name)?.clone())
}
