#[path = "../src/peer_runner_client.rs"]
mod peer_runner_client;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::post,
};
use peer_runner_client::{ClientError, HostPermissions, PeerRunnerClient};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Captured {
    request: Arc<Mutex<Option<Value>>>,
    authorization: Arc<Mutex<Option<String>>>,
}

async fn operation_handler(
    State(captured): State<Captured>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    *captured.request.lock().await = Some(body);
    *captured.authorization.lock().await = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    Json(json!({"ok":true,"operation":"chain.get","result":{"value":7}}))
}

async fn start(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}/"), server)
}

#[tokio::test]
async fn sends_authenticated_core_envelope_with_host_owned_permissions() {
    let captured = Captured::default();
    let router = Router::new()
        .route("/api/v2/operations", post(operation_handler))
        .with_state(captured.clone());
    let (base, server) = start(router).await;
    let client = PeerRunnerClient::new(
        &base,
        "local-secret-token",
        "peer-runner",
        HostPermissions::new(true, false, false),
    )
    .unwrap();
    let result = client
        .operate("chain.get", Some("chain-123"), &json!({"include":"status"}))
        .await
        .unwrap();
    assert_eq!(result["result"]["value"], 7);
    assert_eq!(
        *captured.authorization.lock().await,
        Some("Bearer local-secret-token".to_owned())
    );
    assert_eq!(
        *captured.request.lock().await,
        Some(json!({
            "operation":"chain.get",
            "context":{"actor":"peer-runner","chain_id":"chain-123","allow_write":true,
                "allow_model":false,"allow_revoke":false},
            "payload":{"include":"status"}
        }))
    );
    server.abort();
}

#[tokio::test]
async fn non_success_body_and_bearer_are_not_reflected_in_errors() {
    let router = Router::new().route(
        "/api/v2/operations",
        post(|| async {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "BODY-SECRET and response-token-secret",
            )
        }),
    );
    let (base, server) = start(router).await;
    let client = PeerRunnerClient::new(
        &base,
        "bearer-secret-that-must-not-leak",
        "actor",
        HostPermissions::new(false, false, false),
    )
    .unwrap();
    let error = client
        .operate("chain.get", None, &json!({}))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::HttpStatus(500));
    let diagnostic = format!("{error:?} {error}");
    for secret in [
        "BODY-SECRET",
        "response-token-secret",
        "bearer-secret-that-must-not-leak",
    ] {
        assert!(!diagnostic.contains(secret));
    }
    server.abort();
}

#[test]
fn rejects_non_loopback_credentials_query_fragment_and_non_root_paths() {
    let permissions = HostPermissions::new(true, true, false);
    assert!(PeerRunnerClient::new("http://[::1]:18912/", "token", "actor", permissions).is_ok());
    for url in [
        "http://8.8.8.8:18912/",
        "http://user:pass@127.0.0.1:18912/",
        "http://@127.0.0.1:18912/",
        "http://127.0.0.1:18912/?token=secret",
        "http://127.0.0.1:18912/#fragment",
        "http://127.0.0.1:18912/base/",
        "https://127.0.0.1:18912/",
    ] {
        assert!(
            PeerRunnerClient::new(url, "token", "actor", permissions).is_err(),
            "{url}"
        );
    }
}

#[tokio::test]
async fn refuses_redirect_without_contacting_redirect_target() {
    let target_called = Arc::new(AtomicBool::new(false));
    let flag = target_called.clone();
    let target_router = Router::new().route(
        "/api/v2/operations",
        post(move || {
            let flag = flag.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Json(json!({"ok":true}))
            }
        }),
    );
    let (target_base, target_server) = start(target_router).await;

    let redirect_router = Router::new().route(
        "/api/v2/operations",
        post(move || async move {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, format!("{target_base}api/v2/operations"))],
                "redirect body secret",
            )
        }),
    );
    let (base, redirect_server) = start(redirect_router).await;
    let client = PeerRunnerClient::new(
        &base,
        "local-token",
        "actor",
        HostPermissions::new(false, false, false),
    )
    .unwrap();
    let error = client
        .operate("chain.get", None, &json!({}))
        .await
        .unwrap_err();
    assert_eq!(error, ClientError::HttpStatus(307));
    assert!(!target_called.load(Ordering::SeqCst));

    redirect_server.abort();
    target_server.abort();
}
