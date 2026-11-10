use serde::Deserialize;
use serde_json::json;
use sic_core_rust::{
    peer_supervisor::{self, SupervisorConfig},
    peer_watch::{CycleOutcome, PeerWatch, WatchConfig, WatchError},
};
use std::{collections::HashSet, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchFile {
    /// A missing or false flag never starts the polling service.
    #[serde(default)]
    enabled: bool,
    namespace: String,
    poll_interval_ms: u64,
    max_tasks_per_cycle: usize,
    page_size: usize,
    supervisor: SupervisorConfig,
}

fn stopped(code: &str, exit_code: i32) -> ! {
    eprintln!("{}", json!({"state":"stopped", "code":code}));
    std::process::exit(exit_code);
}

fn read_bearer(config: &SupervisorConfig) -> String {
    match std::env::var(&config.bearer_env) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => stopped("bearer_unavailable", 2),
    }
}

fn watch_error(error: WatchError) -> &'static str {
    match error {
        WatchError::InvalidConfiguration => "invalid_configuration",
        WatchError::CoreUnavailable => "core_unavailable",
        WatchError::InvalidCoreResponse => "invalid_core_response",
        WatchError::StorageUnavailable => "storage_unavailable",
    }
}

async fn run_once(bytes: &[u8]) {
    let config: SupervisorConfig =
        serde_json::from_slice(bytes).unwrap_or_else(|_| stopped("invalid_configuration", 2));
    let bearer = read_bearer(&config);
    match peer_supervisor::run(&config, bearer).await {
        Ok(outcome) => match serde_json::to_string(&outcome) {
            Ok(json) => println!("{json}"),
            Err(_) => stopped("output_failed", 1),
        },
        Err(error) => stopped(error.0, 1),
    }
}

async fn watch(bytes: &[u8]) {
    let file: WatchFile =
        serde_json::from_slice(bytes).unwrap_or_else(|_| stopped("invalid_configuration", 2));
    if !file.enabled {
        stopped("watch_not_enabled", 2);
    }
    // Validate the complete trusted template before reading credentials or
    // claiming work. Task-specific identities are replaced from Core later.
    peer_supervisor::validate(&file.supervisor)
        .unwrap_or_else(|_| stopped("invalid_configuration", 2));
    let bearer = read_bearer(&file.supervisor);
    let interval = Duration::from_millis(file.poll_interval_ms);
    let config = WatchConfig {
        enabled: true,
        namespace: file.namespace,
        poll_interval: interval,
        max_tasks_per_cycle: file.max_tasks_per_cycle,
        page_size: file.page_size,
    };
    let mut watcher = PeerWatch::new(config, file.supervisor, bearer)
        .await
        .unwrap_or_else(|error| stopped(watch_error(error), 2));
    let (stop, mut stopped_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop.send(true);
        }
    });
    println!("{}", json!({"state":"watching"}));
    let mut reported_blocked = HashSet::new();
    loop {
        if *stopped_rx.borrow() {
            break;
        }
        // Do not interrupt an active DSH execution on Ctrl-C. Stop after its
        // durable outcome is recorded, then leave queued work untouched.
        match watcher.cycle().await {
            Ok(CycleOutcome::Disabled) => stopped("watch_not_enabled", 1),
            Ok(CycleOutcome::Idle) => {}
            Ok(CycleOutcome::Replied {
                task_id,
                reply_message_id,
            }) => println!(
                "{}",
                json!({"state":"reply_delivered","task_id":task_id,
                    "reply_message_id":reply_message_id,"requester_adopted_cb":false})
            ),
            Ok(CycleOutcome::ReplyAlreadyPresent { task_id }) => {
                println!("{}", json!({"state":"reply_present","task_id":task_id}))
            }
            Ok(CycleOutcome::Blocked {
                task_id,
                revision,
                reason,
            }) => {
                if reported_blocked.len() >= 4096 {
                    reported_blocked.clear();
                }
                if reported_blocked.insert((task_id.clone(), revision, reason)) {
                    println!(
                        "{}",
                        json!({"state":"blocked","task_id":task_id,"revision":revision,"reason":reason})
                    );
                }
            }
            Err(error) => stopped(watch_error(error), 1),
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {},
            _ = stopped_rx.changed() => {},
        }
    }
    println!("{}", json!({"state":"stopped","code":"requested"}));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 || !["run", "watch"].contains(&args[1].as_str()) || args[2] != "--config" {
        stopped("usage", 2);
    }
    let bytes = match std::fs::read(&args[3]) {
        Ok(bytes) if bytes.len() <= 1_048_576 => bytes,
        _ => stopped("config_unavailable", 2),
    };
    if args[1] == "run" {
        run_once(&bytes).await;
    } else {
        watch(&bytes).await;
    }
}
