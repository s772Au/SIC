use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::tempdir;

#[test]
fn dsh_worker_call_limit_rejects_invalid_configuration_before_opening_database() {
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    for value in ["0", "65", "not-a-number"] {
        let result = Command::new(binary)
            .args([
                "serve",
                "nonexistent.db",
                "127.0.0.1:0",
                "owner",
                "SIC_TEST_BEARER",
                "--dsh-alias",
                "sic-worker",
                "--dsh-max-worker-calls",
                value,
            ])
            .env("SIC_TEST_BEARER", "test-only-token")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("must be an integer in 1..=64"));
    }
    for value in ["16", "64"] {
        let result = Command::new(binary)
            .args([
                "serve",
                "nonexistent.db",
                "127.0.0.1:0",
                "owner",
                "SIC_TEST_BEARER",
                "--dsh-max-worker-calls",
                value,
            ])
            .env("SIC_TEST_BEARER", "test-only-token")
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "an accepted value may only fail on the missing alias"
        );
        assert!(String::from_utf8_lossy(&result.stderr).contains("requires --dsh-alias"));
    }

    for value in ["-1", "3", "invalid"] {
        let result = Command::new(binary)
            .args([
                "serve",
                "nonexistent.db",
                "127.0.0.1:0",
                "owner",
                "SIC_TEST_BEARER",
                "--dsh-alias",
                "sic-worker",
                "--dsh-invalid-choice-retries",
                value,
            ])
            .env("SIC_TEST_BEARER", "test-only-token")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("must be an integer in 0..=2"));
    }
}

#[test]
fn service_database_lock_releases_after_process_termination() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("owned.db");
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    let init = Command::new(binary)
        .args(["init", db.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success());
    let start = || {
        Command::new(binary)
            .args([
                "serve",
                db.to_str().unwrap(),
                "127.0.0.1:0",
                "owner",
                "SIC_TEST_BEARER",
            ])
            .env("SIC_TEST_BEARER", "test-only-token")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut first = start();
    let mut line = String::new();
    BufReader::new(first.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("listening on"));
    let second = Command::new(binary)
        .args([
            "serve",
            db.to_str().unwrap(),
            "127.0.0.1:0",
            "owner",
            "SIC_TEST_BEARER",
        ])
        .env("SIC_TEST_BEARER", "test-only-token")
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already owns"));
    first.kill().unwrap();
    first.wait().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let mut third = start();
    let mut line = String::new();
    BufReader::new(third.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("listening on"));
    third.kill().unwrap();
    third.wait().unwrap();
}

#[test]
fn unverified_database_does_not_gain_a_service_lock() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("unverified.db");
    std::fs::write(&db, b"not a SIC Rust database").unwrap();
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    let output = Command::new(binary)
        .args([
            "serve",
            db.to_str().unwrap(),
            "127.0.0.1:0",
            "owner",
            "SIC_TEST_BEARER",
        ])
        .env("SIC_TEST_BEARER", "test-only-token")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!dir.path().join("unverified.db.service.lock").exists());
    assert_eq!(std::fs::read(&db).unwrap(), b"not a SIC Rust database");
}

#[cfg(windows)]
#[test]
fn service_lock_is_checked_before_opening_database_for_writes() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tempdir().unwrap();
    let db = dir.path().join("guarded.db");
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    assert!(
        Command::new(binary)
            .args(["init", db.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.path().join("guarded.db.service.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    // Permit the read-only schema probe, but deny another write-open. If the
    // process opens the database for writes before testing the lock, it fails
    // with a storage error instead of reporting the already-owned lock.
    let _read_only = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x00000001) // FILE_SHARE_READ
        .open(&db)
        .unwrap();
    let output = Command::new(binary)
        .args([
            "serve",
            db.to_str().unwrap(),
            "127.0.0.1:0",
            "owner",
            "SIC_TEST_BEARER",
        ])
        .env("SIC_TEST_BEARER", "test-only-token")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("already owns"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn dsh_worker_call_limit_can_be_read_from_the_model_config() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("config-limit.db");
    let binary = env!("CARGO_BIN_EXE_sic-core-rust");
    assert!(
        Command::new(binary)
            .args(["init", db.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let profile = serde_json::json!({
        "model": "openai/deepseek-v4.1-flash-eco",
        "expected_returned_models": ["deepseek-v4.1-flash-eco"],
        "api_base": "http://127.0.0.1:9/v1",
        "api_key_env": "SIC_TEST_PROVIDER_KEY",
        "tokenizer_model": "host-counter-gpt4o",
        "tokenizer_status": "host-selected-surrogate-not-provider-attested",
        "max_input_tokens": 1_000_000,
        "max_output_tokens": 393_216,
        "default_output_tokens": 4096,
        "max_supported_output_tokens": 393_216,
        "context_window": 1_000_000,
        "timeout_seconds": 5
    });
    let config_path = dir.path().join("model.json");
    let write = |limit: serde_json::Value| {
        std::fs::write(
            &config_path,
            serde_json::to_vec(&serde_json::json!({
                "profiles": {"fixture": profile},
                "roles": {"worker": "fixture", "editor": "fixture"},
                "dsh_bridge": {"max_worker_calls_per_turn": limit},
            }))
            .unwrap(),
        )
        .unwrap();
    };
    write(serde_json::json!(65));
    let rejected = Command::new(binary)
        .args([
            "serve",
            db.to_str().unwrap(),
            "127.0.0.1:0",
            "owner",
            "SIC_TEST_BEARER",
            "--allow-model",
            "--model-config",
            config_path.to_str().unwrap(),
            "--dsh-alias",
            "sic-worker",
        ])
        .env("SIC_TEST_BEARER", "test-only-token")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("dsh_bridge.max_worker_calls_per_turn must be an integer in 1..=64")
    );

    write(serde_json::json!(12));
    let mut accepted = Command::new(binary)
        .args([
            "serve",
            db.to_str().unwrap(),
            "127.0.0.1:0",
            "owner",
            "SIC_TEST_BEARER",
            "--allow-model",
            "--model-config",
            config_path.to_str().unwrap(),
            "--dsh-alias",
            "sic-worker",
        ])
        .env("SIC_TEST_BEARER", "test-only-token")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(accepted.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    accepted.kill().unwrap();
    accepted.wait().unwrap();
    assert!(line.contains("listening on"), "unexpected stdout: {line}");
}
