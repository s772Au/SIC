//! Provider-reported usage, grouped without conflating roles or model routes.
use crate::{
    error::{CoreError, Result},
    storage,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Only non-secret route identity is persisted with each model attempt.
pub fn model_identity(profile: &Value) -> Value {
    let base = profile.get("api_base").and_then(Value::as_str);
    let origin = base
        .and_then(|v| reqwest::Url::parse(v).ok())
        .map(|url| url.origin().ascii_serialization());
    json!({
        "requested_model": profile.get("model").and_then(Value::as_str),
        "provider_origin": origin,
        // Distinct route paths on the same origin may be different providers.
        // The path itself can contain sensitive routing information.
        "provider_route_fingerprint": base.map(|v| storage::sha256(v.as_bytes()))
    })
}

#[derive(Default)]
struct Counter {
    known: u64,
    reported: u64,
    missing: u64,
}

impl Counter {
    fn observe(&mut self, value: Option<u64>) -> Result<()> {
        if let Some(value) = value {
            self.known = self
                .known
                .checked_add(value)
                .ok_or_else(|| CoreError::invalid("provider usage overflow"))?;
            self.reported += 1;
        } else {
            self.missing += 1;
        }
        Ok(())
    }
    fn as_json(&self) -> Value {
        json!({"known_tokens":self.known,"reported_attempts":self.reported,
               "unknown_attempts":self.missing,"complete":self.missing == 0 && self.reported > 0})
    }
}

#[derive(Default)]
struct Group {
    attempts: u64,
    prompt: Counter,
    completion: Counter,
    cached_prompt: Counter,
}

// Cached prompt tokens are a subset of prompt_tokens, not an additional bill.
// If two recognized provider fields disagree, keep that attempt unknown.
fn cached_prompt_tokens(usage: &Value) -> Option<u64> {
    let nested = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let direct = usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64);
    match (nested, direct) {
        (Some(a), Some(b)) if a != b => None,
        (Some(a), _) | (_, Some(a)) => Some(a),
        _ => None,
    }
}

/// `attempts` must be the already-authorized Agent's recorded attempts.
/// Unknown identities and missing counters remain explicitly unknown.
pub fn grouped_usage(attempts: &[Value]) -> Result<Vec<Value>> {
    type Key = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let mut groups: BTreeMap<Key, Group> = BTreeMap::new();
    for attempt in attempts {
        let text = |object: &Value, field: &str| {
            object.get(field).and_then(Value::as_str).map(str::to_owned)
        };
        let identity = &attempt["model_identity"];
        let key = (
            text(attempt, "role").unwrap_or_else(|| "unknown".into()),
            text(identity, "provider_origin"),
            text(identity, "provider_route_fingerprint"),
            text(identity, "requested_model"),
            text(attempt, "provider_reported_model"),
        );
        let group = groups.entry(key).or_default();
        group.attempts += 1;
        let usage = &attempt["provider_reported_usage"];
        group
            .prompt
            .observe(usage.get("prompt_tokens").and_then(Value::as_u64))?;
        group
            .completion
            .observe(usage.get("completion_tokens").and_then(Value::as_u64))?;
        let cache = cached_prompt_tokens(usage).filter(|cached| {
            usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .is_none_or(|input| *cached <= input)
        });
        group.cached_prompt.observe(cache)?;
    }
    Ok(groups.into_iter().map(|((role, origin, route, requested, reported), group)| json!({
        "role":role,"provider_origin":origin,"provider_route_fingerprint":route,
        "requested_model":requested,"provider_reported_model":reported,
        "attempts":group.attempts,"prompt_tokens":group.prompt.as_json(),
        "completion_tokens":group.completion.as_json(),"cached_prompt_tokens":group.cached_prompt.as_json(),
        "cache_semantics":"cached_prompt_tokens are included in prompt_tokens",
        "priced_cost":Value::Null
    })).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_roles_routes_models_missing_and_cache_conflicts() {
        let identity = |route| {
            model_identity(
                &json!({"model":"openai/fixed", "api_base":format!("https://example.test/{route}")}),
            )
        };
        let attempts = vec![
            json!({"role":"editor","model_identity":identity("one"),"provider_reported_model":"fixed",
                "provider_reported_usage":{"prompt_tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":7}}}),
            json!({"role":"editor","model_identity":identity("one"),"provider_reported_model":"fixed",
                "provider_reported_usage":{"prompt_tokens":8,"prompt_cache_hit_tokens":2,"prompt_tokens_details":{"cached_tokens":3}}}),
            json!({"role":"worker","model_identity":identity("one"),"provider_reported_model":"fixed","provider_reported_usage":null}),
            json!({"role":"editor","model_identity":identity("two"),"provider_reported_model":"fixed",
                "provider_reported_usage":{"prompt_tokens":3,"completion_tokens":1}}),
        ];
        let groups = grouped_usage(&attempts).unwrap();
        assert_eq!(groups.len(), 3);
        let editor = groups
            .iter()
            .find(|g| g["role"] == "editor" && g["attempts"] == 2)
            .unwrap();
        assert_eq!(editor["prompt_tokens"]["known_tokens"], 18);
        assert_eq!(editor["completion_tokens"]["unknown_attempts"], 1);
        assert_eq!(editor["cached_prompt_tokens"]["known_tokens"], 7);
        assert_eq!(editor["cached_prompt_tokens"]["unknown_attempts"], 1);
        let worker = groups.iter().find(|g| g["role"] == "worker").unwrap();
        assert_eq!(worker["prompt_tokens"]["unknown_attempts"], 1);
        assert_eq!(worker["priced_cost"], Value::Null);
        assert_ne!(
            identity("one")["provider_route_fingerprint"],
            identity("two")["provider_route_fingerprint"]
        );
    }
}
