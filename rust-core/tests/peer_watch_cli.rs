use std::{fs, process::Command};

fn execute(value: &serde_json::Value) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    fs::write(&config, serde_json::to_vec(value).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_sic-peer-supervisor"))
        .args(["watch", "--config"])
        .arg(&config)
        .env_remove("SIC_CORE_TOKEN")
        .output()
        .unwrap()
}

#[test]
fn watch_requires_explicit_opt_in_and_does_not_need_a_bearer_to_refuse() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    for enabled in [Some(false), None] {
        if let Some(enabled) = enabled {
            value["enabled"] = enabled.into();
        } else {
            value.as_object_mut().unwrap().remove("enabled");
        }
        let output = execute(&value);
        assert_eq!(output.status.code(), Some(2));
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "watch_not_enabled");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn watch_rejects_unknown_fields_and_invalid_host_config_before_claiming() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../config/peer-watch.example.json")).unwrap();
    value["enabled"] = true.into();
    // Example paths intentionally do not exist; this must fail before the
    // missing bearer is examined or a task can be claimed.
    let output = execute(&value);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "invalid_configuration");

    value["unexpected"] = "ignored by no one".into();
    let output = execute(&value);
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "invalid_configuration");
}
