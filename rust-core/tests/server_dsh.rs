use axum::Router;
use sic_core_rust::{
    core::Core,
    dsh_bridge::{CoreRuntime, DshBridge, DshBridgeConfig},
    server::{self, HostAuth},
    storage::Store,
};
use std::sync::Arc;

#[tokio::test]
async fn mounted_dsh_route_requires_same_host_bearer_and_model_grant() {
    let store = Store::memory().await.unwrap();
    let core = Core::new(store.clone());
    let bridge = Arc::new(
        DshBridge::new(
            store,
            Arc::new(CoreRuntime::new(core.clone())),
            DshBridgeConfig {
                model_alias: "sic-worker".into(),
                fixed_max_tokens: Some(4096),
                fixed_reasoning_effort: None,
                max_worker_calls_per_turn: 4,
                max_host_tasks_per_response: 4,
                max_harness_tools: 16,
            },
        )
        .await
        .unwrap(),
    );
    let auth = HostAuth::new("fixture-bearer", "owner", true, true, false).unwrap();
    let app: Router = server::router_with_dsh(core, auth, bridge);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://{address}/v1/chat/completions");
    let denied = client
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    let admitted = client
        .post(&url)
        .bearer_auth("fixture-bearer")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_ne!(admitted.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(admitted.headers()["cache-control"], "no-store");
    let status_url = format!("http://{address}/v1/sic/status");
    let denied = client
        .post(&status_url)
        .json(&serde_json::json!({"session_id":"unknown"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    task.abort();
}
