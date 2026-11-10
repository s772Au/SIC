use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::tempdir;

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
