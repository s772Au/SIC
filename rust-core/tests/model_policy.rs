#[path = "../src/model_policy.rs"]
mod model_policy;

use model_policy::{resolve_editor_profile, validate_config};
use serde_json::{Value, json};

fn profile(model: &str, context: u64, max_input: u64, max_output: u64) -> Value {
    json!({
        "model": model,
        "tokenizer_model": "host-counter-model",
        "tokenizer_status": "host-selected-not-provider-attested",
        "max_input_tokens": max_input,
        "max_output_tokens": max_output,
        "default_output_tokens": max_output.min(4096),
        "context_window": context
    })
}

fn config() -> Value {
    json!({
        "profiles": {
            "worker": profile("worker-model", 131072, 100000, 4096),
            "editor-default": profile("editor-model", 131072, 100000, 4096),
            "editor-named": profile("editor-specialist", 200000, 150000, 8192),
            "editor-level": profile("editor-level-model", 1000000, 1000000, 393216)
        },
        "roles": {"worker": "worker", "editor": {"profile": "editor-default"}},
        "editor_profiles": {"editor-2": "editor-named"},
        "editor_profiles_by_level": {"3": "editor-level"}
    })
}

fn python_direct_role_config(include_profiles: bool) -> Value {
    let mut worker = profile(
        "openai/deepseek-v4.1-flash-eco",
        1_000_000,
        1_000_000,
        393_216,
    );
    worker["api_base"] = json!("https://provider.invalid/v1");
    worker["api_key_env"] = json!("SIC_PROVIDER_KEY");
    worker["expected_returned_models"] = json!(["deepseek-v4.1-flash-eco"]);
    worker["default_output_tokens"] = json!(16_384);
    worker["max_supported_output_tokens"] = json!(393_216);
    worker["timeout_seconds"] = json!(600);
    worker["stream_response"] = json!(true);

    let mut editor = worker.clone();
    editor["model"] = json!("openai/editor-model");

    let mut cfg = json!({"roles": {"worker": worker, "editor": editor}});
    if include_profiles {
        cfg["profiles"] = json!({});
    }
    cfg
}

#[test]
fn validates_explicit_profiles_and_allows_worker_editor_model_difference() {
    let cfg = config();
    assert!(validate_config(&cfg).is_ok());
    assert_eq!(cfg["profiles"]["worker"]["model"], "worker-model");
    assert_eq!(cfg["profiles"]["editor-default"]["model"], "editor-model");
}

#[test]
fn example_opendesign_profile_preserves_the_user_supplied_model_limits() {
    let cfg: Value =
        serde_json::from_str(include_str!("../config/opendesign.example.json")).unwrap();
    validate_config(&cfg).unwrap();
    let worker = model_policy::resolve_worker_profile(&cfg).unwrap();
    let editor = resolve_editor_profile(&cfg, 1, "editor-1").unwrap();
    assert_eq!(worker, editor);
    assert_eq!(worker["model"], "openai/deepseek-v4.1-flash-eco");
    assert_eq!(worker["api_base"], "https://amr-link.open-design.ai/v1");
    assert_eq!(worker["context_window"], 1_000_000);
    assert_eq!(worker["max_output_tokens"], 393_216);
    assert_eq!(worker["default_output_tokens"], 4096);
    assert!(worker.get("api_key").is_none());
    sic_core_rust::provider::validate_openai_profile(&worker).unwrap();
}

#[test]
fn resolves_level_before_editor_then_default_role() {
    let cfg = config();
    assert_eq!(
        resolve_editor_profile(&cfg, 3, "editor-2").unwrap()["model"],
        "editor-level-model"
    );
    assert_eq!(
        resolve_editor_profile(&cfg, 2, "editor-2").unwrap()["model"],
        "editor-specialist"
    );
    assert_eq!(
        resolve_editor_profile(&cfg, 2, "editor-1").unwrap()["model"],
        "editor-model"
    );
}

#[test]
fn resolver_returns_a_deep_copy_for_job_freezing() {
    let mut cfg = config();
    cfg["profiles"]["editor-level"]["nested"] = json!({"v": [1, 2]});
    let frozen = resolve_editor_profile(&cfg, 3, "editor-1").unwrap();
    cfg["profiles"]["editor-level"]["model"] = json!("changed-after-freeze");
    cfg["profiles"]["editor-level"]["nested"]["v"][0] = json!(99);
    assert_eq!(frozen["model"], "editor-level-model");
    assert_eq!(frozen["nested"]["v"][0], 1);
}

#[test]
fn rejects_zero_level_and_empty_editor() {
    let cfg = config();
    assert!(resolve_editor_profile(&cfg, 0, "editor-1").is_err());
    assert!(resolve_editor_profile(&cfg, 1, " ").is_err());
}

#[test]
fn invalid_explicit_mapping_does_not_fall_back() {
    let mut cfg = config();
    cfg["editor_profiles_by_level"]["3"] = json!("missing");
    assert!(resolve_editor_profile(&cfg, 3, "editor-2").is_err());
    cfg["editor_profiles_by_level"] = json!({});
    cfg["editor_profiles"]["editor-2"] = json!("missing");
    assert!(resolve_editor_profile(&cfg, 2, "editor-2").is_err());
}

#[test]
fn accepts_one_million_context_and_393216_output_without_128k_cap() {
    let cfg = python_direct_role_config(true);
    assert!(validate_config(&cfg).is_ok());
    let resolved = resolve_editor_profile(&cfg, 3, "editor-1").unwrap();
    assert_eq!(resolved["model"], "openai/editor-model");
    assert_eq!(resolved["context_window"], 1_000_000);
    assert_eq!(resolved["max_output_tokens"], 393_216);
}

#[test]
fn accepts_python_direct_role_profiles_with_empty_or_missing_registry() {
    let with_empty_registry = python_direct_role_config(true);
    assert!(validate_config(&with_empty_registry).is_ok());
    assert_eq!(
        resolve_editor_profile(&with_empty_registry, 1, "editor-1").unwrap()["api_key_env"],
        "SIC_PROVIDER_KEY"
    );

    let without_registry = python_direct_role_config(false);
    assert!(validate_config(&without_registry).is_ok());
    assert_eq!(
        resolve_editor_profile(&without_registry, 1, "editor-1").unwrap()["model"],
        "openai/editor-model"
    );
}

#[test]
fn direct_editor_role_profile_is_deep_copied_for_freezing() {
    let mut cfg = python_direct_role_config(true);
    cfg["roles"]["editor"]["nested"] = json!({"v": [1, 2]});
    let frozen = resolve_editor_profile(&cfg, 1, "editor-1").unwrap();
    cfg["roles"]["editor"]["model"] = json!("changed-after-freeze");
    cfg["roles"]["editor"]["nested"]["v"][0] = json!(99);
    assert_eq!(frozen["model"], "openai/editor-model");
    assert_eq!(frozen["nested"]["v"][0], 1);
}

#[test]
fn rejects_unknown_profiles_and_missing_default_role() {
    let mut cfg = config();
    cfg["editor_profiles"]["new-editor"] = json!("not-registered");
    assert!(validate_config(&cfg).is_err());
    cfg = config();
    cfg["roles"].as_object_mut().unwrap().remove("editor");
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn rejects_invalid_context_and_output_budgets() {
    for bad in [
        json!({"context_window":0}),
        json!({"context_window":131072,"max_input_tokens":131073}),
        json!({"context_window":131072,"max_output_tokens":131073}),
        json!({"default_output_tokens":0}),
        json!({"default_output_tokens":8193}),
        json!({"max_supported_output_tokens":4095}),
        json!({"max_output_tokens":1.5}),
    ] {
        let mut cfg = config();
        for (key, value) in bad.as_object().unwrap() {
            cfg["profiles"]["editor-default"][key] = value.clone();
        }
        assert!(
            validate_config(&cfg).is_err(),
            "accepted invalid override: {bad}"
        );
    }
}

#[test]
fn rejects_missing_required_profile_fields_and_inline_secret_fields() {
    let mut cfg = config();
    cfg["profiles"]["editor-default"] = json!({"model":"m"});
    assert!(validate_config(&cfg).is_err());
    cfg = config();
    cfg["profiles"]["editor-default"]["api_key"] = json!("not-read");
    assert!(validate_config(&cfg).is_err());
}

#[test]
fn does_not_infer_a_model_from_level_or_name() {
    let mut cfg = config();
    cfg["editor_profiles_by_level"] = json!({});
    cfg["editor_profiles"] = json!({});
    assert_eq!(
        resolve_editor_profile(&cfg, 99, "editor-99").unwrap()["model"],
        "editor-model"
    );
}
