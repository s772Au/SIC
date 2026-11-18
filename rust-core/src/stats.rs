//! Session statistics over host-measured attempts.
//!
//! This module is a pure projection. It never calls a model, never prices
//! anything, and never turns a missing counter into zero. It answers the
//! questions a status display asks -- how many turns and steps ran, how fast
//! they streamed, how much prompt came from cache, how full the context is --
//! using only provider-reported usage and host-measured wall-clock intervals.
//!
//! Two deliberate differences from a streaming harness's statistics:
//!
//! * Every SIC model call is one non-streaming request, so there is no
//!   time-to-first-token to subtract. `tokens_per_second` here is the whole
//!   call's output divided by its full wall time, and it is labelled as such
//!   rather than presented as a decode rate.
//! * A SIC turn boundary belongs to the host bridge. When the bridge tables
//!   are absent the turn count stays `null` instead of being guessed from
//!   model-call counts.

use serde_json::{Value, json};

/// Provider-reported usage fields that SIC records per attempt.
const PROMPT: &str = "/provider_reported_usage/prompt_tokens";
const COMPLETION: &str = "/provider_reported_usage/completion_tokens";
const CACHED: &str = "/provider_reported_cached_tokens";

fn at(value: &Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(Value::as_u64)
}

fn ratio(numerator: Option<u64>, denominator: Option<u64>) -> Option<f64> {
    match (numerator, denominator) {
        (Some(n), Some(d)) if d > 0 => Some(n as f64 / d as f64),
        _ => None,
    }
}

/// Sum one counter across attempts, tracking how many attempts reported it.
/// A partial sum stays usable but is marked incomplete rather than treated as a
/// complete total.
fn tally(attempts: &[Value], pointer: &str) -> Value {
    let mut known = 0u64;
    let mut reported = 0u64;
    let mut missing = 0u64;
    for attempt in attempts {
        match at(attempt, pointer) {
            Some(value) => {
                known = known.saturating_add(value);
                reported += 1;
            }
            None => missing += 1,
        }
    }
    json!({
        "known": known,
        "reported_attempts": reported,
        "unknown_attempts": missing,
        "complete": missing == 0 && reported > 0,
    })
}

/// Interval union in seconds. Intervals may overlap, so their sum is not the
/// span; the caller gets both figures and no invented wall clock.
fn union_seconds(intervals: &[Value]) -> u64 {
    let mut spans: Vec<(u64, u64)> = intervals
        .iter()
        .filter_map(|interval| Some((at(interval, "/start")?, at(interval, "/end")?)))
        .filter(|(start, end)| end > start)
        .collect();
    if spans.is_empty() {
        return 0;
    }
    spans.sort_unstable();
    let mut total = 0u64;
    let (mut start, mut end) = spans[0];
    for (next_start, next_end) in spans.into_iter().skip(1) {
        if next_start <= end {
            end = end.max(next_end);
        } else {
            total = total.saturating_add(end - start);
            start = next_start;
            end = next_end;
        }
    }
    total.saturating_add(end - start)
}

fn summed(intervals: &[Value]) -> Option<u64> {
    let mut any = false;
    let mut total = 0u64;
    for interval in intervals {
        if let Some(seconds) = at(interval, "/seconds") {
            total = total.saturating_add(seconds);
            any = true;
        }
    }
    any.then_some(total)
}

/// Everything the projection needs. All fields are already host-measured; this
/// struct adds no new observations.
pub struct Observation<'a> {
    pub attempts: &'a [Value],
    pub intervals: &'a [Value],
    /// Capacity of the model route the latest run was bound to, when known.
    pub context_window: Option<u64>,
    /// User-turn count owned by the host bridge, when its tables are present.
    pub turns: Option<u64>,
}

pub fn project(observation: Observation<'_>) -> Value {
    let attempts = observation.attempts;
    let intervals = observation.intervals;

    let mut worker_steps = 0u64;
    let mut editor_steps = 0u64;
    for attempt in attempts {
        match attempt.get("role").and_then(Value::as_str) {
            Some("worker") => worker_steps += 1,
            Some("editor") => editor_steps += 1,
            _ => {}
        }
    }

    let prompt = tally(attempts, PROMPT);
    let output = tally(attempts, COMPLETION);
    let cache = tally(attempts, CACHED);

    let prompt_total = prompt["known"].as_u64().unwrap_or(0);
    let output_total = output["known"].as_u64().unwrap_or(0);
    let cache_total = cache["known"].as_u64().unwrap_or(0);

    let model_seconds = summed(intervals);
    let span_seconds = union_seconds(intervals);
    let wall_seconds = match (
        intervals.iter().filter_map(|i| at(i, "/start")).min(),
        intervals.iter().filter_map(|i| at(i, "/end")).max(),
    ) {
        (Some(start), Some(end)) if end > start => Some(end - start),
        _ => None,
    };

    // Cache granularity: SIC records the provider's `cached_tokens`, which is a
    // SUBSET of `prompt_tokens` rather than a separate bucket. The share is
    // therefore read/cached over the same prompt total that already contains it.
    let cache_hit_rate = ratio(
        (!cache["complete"].as_bool().unwrap_or(false)).then_some(0).or_else(|| Some(cache_total)),
        Some(prompt_total),
    )
    .filter(|_| prompt_total > 0);

    let tokens_per_second = ratio(
        Some(output_total),
        model_seconds.filter(|seconds| *seconds > 0),
    );

    // Occupancy answers for the NEXT request, so it uses the newest completed
    // prompt rather than a sum. Attempts are appended in completion order.
    let context_tokens = attempts.iter().rev().find_map(|attempt| at(attempt, PROMPT));
    let context_percent = match (context_tokens, observation.context_window) {
        (Some(tokens), Some(window)) if window > 0 => Some(tokens as f64 / window as f64),
        _ => None,
    };

    let mut unknowns = Vec::new();
    if !prompt["complete"].as_bool().unwrap_or(false) {
        unknowns.push("prompt_tokens_incomplete");
    }
    if !output["complete"].as_bool().unwrap_or(false) {
        unknowns.push("output_tokens_incomplete");
    }
    if !cache["complete"].as_bool().unwrap_or(false) {
        unknowns.push("cache_read_tokens_incomplete");
    }
    if observation.turns.is_none() {
        unknowns.push("turn_count_owned_by_host_bridge");
    }
    if observation.context_window.is_none() {
        unknowns.push("context_window_not_bound");
    }
    if model_seconds.is_none() {
        unknowns.push("no_completed_model_interval");
    }

    json!({
        "class": "host_measured",
        "basis": {
            "attempts": "provider_reported_usage",
            "timing": "host_wall_clock_seconds",
            "cache": "provider_reported_cached_prompt_tokens_subset_of_prompt",
            "occupancy": "newest_completed_prompt_over_route_context_window",
        },
        "turns": observation.turns,
        "steps": attempts.len() as u64,
        "steps_by_role": {"worker": worker_steps, "editor": editor_steps},
        "prompt_tokens": prompt,
        "output_tokens": output,
        "cache_read_tokens": cache,
        "total_tokens": {
            "known": prompt_total.saturating_add(output_total),
            "complete": prompt["complete"].as_bool().unwrap_or(false)
                && output["complete"].as_bool().unwrap_or(false),
        },
        "cache_hit_rate": cache_hit_rate,
        "tokens_per_second": tokens_per_second,
        "tokens_per_second_class": "whole_call_output_over_model_seconds_not_decode",
        "model_seconds": model_seconds,
        "model_span_seconds": span_seconds,
        "wall_seconds": wall_seconds,
        "context_tokens": context_tokens,
        "context_window": observation.context_window,
        "context_percent": context_percent,
        "unknowns": unknowns,
        "non_claims": [
            "no_price_or_bill",
            "no_decode_rate_without_streaming",
            "no_physical_cache_hit_proof",
            "no_task_quality_score",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(role: &str, prompt: Option<u64>, output: Option<u64>, cached: Option<u64>) -> Value {
        let mut usage = serde_json::Map::new();
        if let Some(value) = prompt {
            usage.insert("prompt_tokens".into(), json!(value));
        }
        if let Some(value) = output {
            usage.insert("completion_tokens".into(), json!(value));
        }
        json!({
            "role": role,
            "provider_reported_usage": if usage.is_empty() { Value::Null } else { Value::Object(usage) },
            "provider_reported_cached_tokens": cached,
        })
    }

    fn interval(role: &str, start: u64, end: u64) -> Value {
        json!({"role": role, "start": start, "end": end, "seconds": end - start})
    }

    #[test]
    fn reports_steps_throughput_cache_share_and_occupancy() {
        let attempts = vec![
            attempt("worker", Some(5_860), Some(469), Some(4_096)),
            attempt("editor", Some(1_200), Some(120), Some(1_024)),
        ];
        let intervals = vec![interval("worker", 10, 20), interval("editor", 21, 24)];
        let stats = project(Observation {
            attempts: &attempts,
            intervals: &intervals,
            context_window: Some(1_000_000),
            turns: Some(4),
        });
        assert_eq!(stats["turns"], 4);
        assert_eq!(stats["steps"], 2);
        assert_eq!(stats["steps_by_role"]["worker"], 1);
        assert_eq!(stats["steps_by_role"]["editor"], 1);
        assert_eq!(stats["prompt_tokens"]["known"], 7_060);
        assert_eq!(stats["output_tokens"]["known"], 589);
        assert_eq!(stats["total_tokens"]["known"], 7_649);
        assert_eq!(stats["cache_read_tokens"]["known"], 5_120);
        // 5120 / 7060 within a small tolerance.
        let hit = stats["cache_hit_rate"].as_f64().unwrap();
        assert!((hit - 0.7252).abs() < 0.001, "{hit}");
        // 589 output over 13 model seconds.
        let tps = stats["tokens_per_second"].as_f64().unwrap();
        assert!((tps - 45.307).abs() < 0.01, "{tps}");
        assert_eq!(stats["model_seconds"], 13);
        // Occupancy uses the newest prompt, not the sum.
        assert_eq!(stats["context_tokens"], 1_200);
        let percent = stats["context_percent"].as_f64().unwrap();
        assert!((percent - 0.0012).abs() < 1e-9);
        assert_eq!(stats["unknowns"], json!([]));
    }

    #[test]
    fn missing_counters_stay_unknown_and_never_become_zero() {
        let attempts = vec![
            attempt("worker", Some(100), Some(20), Some(80)),
            attempt("editor", None, None, None),
        ];
        let stats = project(Observation {
            attempts: &attempts,
            intervals: &[],
            context_window: None,
            turns: None,
        });
        assert_eq!(stats["steps"], 2);
        assert_eq!(stats["prompt_tokens"]["known"], 100);
        assert_eq!(stats["prompt_tokens"]["unknown_attempts"], 1);
        assert_eq!(stats["prompt_tokens"]["complete"], false);
        assert_eq!(stats["output_tokens"]["complete"], false);
        assert_eq!(stats["cache_read_tokens"]["complete"], false);
        assert_eq!(stats["turns"], Value::Null);
        assert_eq!(stats["context_window"], Value::Null);
        assert_eq!(stats["context_percent"], Value::Null);
        assert_eq!(stats["tokens_per_second"], Value::Null);
        let unknowns = stats["unknowns"].as_array().unwrap();
        assert!(unknowns.contains(&json!("turn_count_owned_by_host_bridge")));
        assert!(unknowns.contains(&json!("context_window_not_bound")));
        assert!(unknowns.contains(&json!("no_completed_model_interval")));
    }

    #[test]
    fn overlapping_intervals_are_unioned_not_summed() {
        let intervals = vec![
            interval("worker", 10, 30),
            interval("editor", 20, 40),
            interval("editor", 45, 50),
        ];
        let stats = project(Observation {
            attempts: &[],
            intervals: &intervals,
            context_window: None,
            turns: None,
        });
        // Sum of the three spans is 45s; the union is 10..40 plus 45..50 = 35s.
        assert_eq!(stats["model_seconds"], 45);
        assert_eq!(stats["model_span_seconds"], 35);
        assert_eq!(stats["wall_seconds"], 40);
    }
}
