//! Local service entry point. A Python database is never opened for writing.
use sic_core_rust::{
    archive::snapshot_python_database,
    core::Core,
    dsh_bridge::{CoreRuntime, DshBridge, DshBridgeConfig},
    legacy_activation::{
        activate_verified_owner_board, audit_quarantined_activation, start_activated_agent,
    },
    legacy_config::{TargetGuard, audit_legacy_config, restore_legacy_config},
    legacy_import::import_stopped_text_backlog,
    legacy_upgrade::upgrade_active_python_agent,
    model_policy,
    server::{self, HostAuth},
    storage::Store,
};
use std::{
    env,
    fs::OpenOptions,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Use the same OS lock for a service and any offline migration command.
fn lock_database(path: &str) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    let mut lock_name = std::fs::canonicalize(path)?.into_os_string();
    lock_name.push(".service.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(PathBuf::from(lock_name))?;
    lock.try_lock()
        .map_err(|_| "another Rust process already owns this database")?;
    Ok(lock)
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("SIC Rust: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("init") if args.len() == 3 => {
            let store = Store::create(Path::new(&args[2])).await?;
            store.close().await;
            println!("Created independent Rust core database.");
        }
        Some("snapshot-python") if args.len() == 4 => {
            let report = snapshot_python_database(Path::new(&args[2]), Path::new(&args[3])).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Some("import-stopped-text") if args.len() == 4 => {
            let report=import_stopped_text_backlog(Path::new(&args[2]),Path::new(&args[3])).await?;
            println!("{}",serde_json::to_string_pretty(&report)?);
        }
        Some("upgrade-active-python") if args.len() == 4 => {
            let report=upgrade_active_python_agent(Path::new(&args[2]),Path::new(&args[3])).await?;
            println!("{}",serde_json::to_string_pretty(&report)?);
        }
        Some("legacy-audit") if args.len() == 5 => {
            let _lock=lock_database(&args[2])?;
            let store=Store::open(Path::new(&args[2])).await?;
            let report=audit_quarantined_activation(&store,&args[3],&args[4]).await?;
            println!("{}",serde_json::to_string_pretty(&report)?);
            store.close().await;
        }
        Some("legacy-activate-board") if args.len() == 7 => {
            let _lock=lock_database(&args[2])?;
            let store=Store::open(Path::new(&args[2])).await?;
            let revision=args[5].parse::<u64>()?;
            let report=activate_verified_owner_board(&store,&args[3],&args[4],revision,&args[6]).await?;
            println!("{}",serde_json::to_string_pretty(&report)?);
            store.close().await;
        }
        Some("legacy-start") if args.len() == 6 => {
            let _lock=lock_database(&args[2])?;
            let store=Store::open(Path::new(&args[2])).await?;
            let report=start_activated_agent(&store,&args[3],&args[4],&args[5]).await?;
            println!("{}",serde_json::to_string_pretty(&report)?);
            store.close().await;
        }
        Some("legacy-config-audit") if args.len() == 5 => {
            let _lock = lock_database(&args[2])?;
            let store = Store::open(Path::new(&args[2])).await?;
            let report = audit_legacy_config(&store, &args[4], &args[3]).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            store.close().await;
        }
        Some("legacy-config-restore") if args.len() == 11 => {
            let _lock = lock_database(&args[2])?;
            let store = Store::open(Path::new(&args[2])).await?;
            let expected = TargetGuard {
                run_state: args[6].clone(),
                cut_seq: args[7].parse::<u64>()?,
                control_epoch: args[8].parse::<u64>()?,
                input_epoch: args[9].parse::<u64>()?,
                config_sha256: args[10].clone(),
            };
            let report = restore_legacy_config(
                &store,
                &args[4],
                &args[3],
                &args[5],
                &expected,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            store.close().await;
        }
        Some("serve") if args.len() >= 6 => {
            let address: SocketAddr = args[3].parse()?;
            if !address.ip().is_loopback() {
                return Err("only an explicit loopback listen address is supported".into());
            }
            let token = env::var(&args[5]).map_err(|_| "configured bearer-token environment variable is missing")?;
            let mut allow_model = false;
            let mut allow_revoke = false;
            let mut model_config: Option<String> = None;
            let mut dsh_alias: Option<String> = None;
            let mut index = 6;
            while index < args.len() {
                match args[index].as_str() {
                    "--allow-model" => allow_model = true,
                    "--allow-revoke" => allow_revoke = true,
                    "--model-config" => {
                        index += 1;
                        model_config = Some(args.get(index).ok_or("missing model config path")?.clone());
                    }
                    "--dsh-alias" => {
                        index += 1;
                        dsh_alias=Some(args.get(index).ok_or("missing DSH public model alias")?.clone());
                    }
                    _ => return Err("unknown serve option".into()),
                }
                index += 1;
            }
            let auth = HostAuth::new(token, args[4].clone(), true, allow_model, allow_revoke)?;
            let store = Store::open(Path::new(&args[2])).await?;
            // The OS releases this lock on exit or crash. It prevents a second
            // service process from treating live bridge receipts as abandoned.
            let lock=lock_database(&args[2])?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            let configuration:Option<serde_json::Value>=model_config.map(|path|
                std::fs::read(path).map_err(|_|"cannot read model configuration")
                    .and_then(|bytes|serde_json::from_slice(&bytes).map_err(|_|"invalid model configuration JSON"))).transpose()?;
            let worker=configuration.as_ref().map(model_policy::resolve_worker_profile).transpose()?;
            let core = match configuration {
                Some(config) => Core::with_model_config(store, config)?,
                None => Core::new(store),
            };
            let app=if let Some(alias)=dsh_alias {
                if !allow_model {return Err("DSH bridge requires the host model grant".into());}
                let worker=worker.ok_or("DSH bridge requires a model configuration")?;
                let bridge=DshBridge::new(core.store.clone(),Arc::new(CoreRuntime::new(core.clone())),DshBridgeConfig {
                    model_alias:alias,
                    fixed_max_tokens:worker["default_output_tokens"].as_u64().or_else(||worker["max_output_tokens"].as_u64()),
                    fixed_reasoning_effort:worker["reasoning_effort"].as_str().map(str::to_owned),
                    // The discoverable catalog may exceed 128; ToolSet applies
                    // the 128 limit only to simultaneously expanded tools.
                    max_worker_calls_per_turn:8,max_host_tasks_per_response:8,max_harness_tools:4096,
                }).await.map_err(|error|format!("DSH bridge setup failed: {}",error.message))?;
                server::router_with_dsh(core,auth,Arc::new(bridge))
            } else {server::router(core,auth)};
            println!("SIC Rust core listening on {}", listener.local_addr()?);
            axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
            drop(lock);
        }
        _ => return Err("usage: sic-core-rust init DB | snapshot-python SOURCE DEST | import-stopped-text SOURCE DEST | upgrade-active-python SOURCE DEST | legacy-audit DB CHAIN ACTOR | legacy-activate-board DB CHAIN ACTOR EXPECTED_REVISION KEY | legacy-start DB CHAIN ACTOR KEY | legacy-config-audit DB CHAIN ACTOR | legacy-config-restore DB CHAIN ACTOR KEY RUN_STATE CUT_SEQ CONTROL_EPOCH INPUT_EPOCH CONFIG_SHA256 | serve DB LOOPBACK:PORT ACTOR TOKEN_ENV [--allow-model --model-config FILE --dsh-alias NAME] [--allow-revoke]".into()),
    }
    Ok(())
}
