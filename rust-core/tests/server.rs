pub mod core {
    pub use sic_core_rust::core::*;
}
pub mod error {
    pub use sic_core_rust::error::*;
}
pub mod dsh_bridge {
    pub use sic_core_rust::dsh_bridge::*;
}

#[path = "../src/server.rs"]
mod server;

use axum::Router;
use serde_json::{Value, json};
use server::{HostAuth, MAX_REQUEST_BODY_BYTES};
use sic_core_rust::{core::Core, storage::Store};
use std::net::SocketAddr;
use tokio::{net::TcpListener, task::JoinHandle};

const TOKEN: &str = "loopback-test-secret";
const ACTOR: &str = "host-owner";

async fn start(core: Core, auth: HostAuth) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let app: Router = server::router(core, auth);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn core() -> Core {
    Core::new(Store::memory().await.unwrap())
}

fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn auth(write: bool, model: bool, revoke: bool) -> HostAuth {
    HostAuth::new(TOKEN, ACTOR, write, model, revoke).unwrap()
}

fn envelope(
    operation: &str,
    actor: &str,
    chain_id: Option<&str>,
    allow_write: bool,
    allow_model: bool,
    allow_revoke: bool,
    payload: Value,
) -> Value {
    json!({
        "operation":operation,
        "context":{
            "actor":actor,
            "chain_id":chain_id,
            "allow_write":allow_write,
            "allow_model":allow_model,
            "allow_revoke":allow_revoke
        },
        "payload":payload
    })
}

async fn post(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    token: Option<&str>,
) -> reqwest::Response {
    let request = client.post(format!("{url}/api/v2/operations"));
    let request = if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    };
    request.json(body).send().await.unwrap()
}

#[tokio::test]
async fn all_routes_require_bearer_and_errors_do_not_echo_or_log_secrets() {
    let (url, task) = start(core().await, auth(true, true, true)).await;
    let client = loopback_client();
    let health = client.get(format!("{url}/health")).send().await.unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(health.headers()[reqwest::header::CACHE_CONTROL], "no-store");
    let capabilities = client
        .get(format!("{url}/api/v2/capabilities"))
        .send()
        .await
        .unwrap();
    assert_eq!(capabilities.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body = envelope("chain.create", ACTOR, None, true, false, false, json!({}));
    let operation = post(&client, &url, &body, None).await;
    assert_eq!(operation.status(), reqwest::StatusCode::UNAUTHORIZED);

    let invalid = post(&client, &url, &body, Some("wrong-canary-secret")).await;
    assert_eq!(invalid.status(), reqwest::StatusCode::UNAUTHORIZED);
    let text = invalid.text().await.unwrap();
    assert!(!text.contains("wrong-canary-secret"));
    assert!(!text.contains(TOKEN));
    task.abort();
}

#[tokio::test]
async fn actor_must_match_host_identity_even_with_valid_bearer() {
    let (url, task) = start(core().await, auth(true, false, false)).await;
    let client = loopback_client();
    let response = post(
        &client,
        &url,
        &envelope(
            "chain.create",
            "attacker",
            None,
            true,
            true,
            true,
            json!({}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["error"]["code"], "actor_mismatch");
    task.abort();
}

#[tokio::test]
async fn client_true_cannot_raise_host_write_or_revoke_permissions() {
    let (url, task) = start(core().await, auth(false, true, false)).await;
    let client = loopback_client();
    let create = post(
        &client,
        &url,
        &envelope(
            "chain.create",
            ACTOR,
            None,
            true,
            true,
            true,
            json!({"creators":[]}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::FORBIDDEN);
    let value: Value = create.json().await.unwrap();
    assert_eq!(value["error"]["code"], "write_denied");
    task.abort();

    let (url, task) = start(core().await, auth(true, true, false)).await;
    let client = loopback_client();
    let create = post(
        &client,
        &url,
        &envelope(
            "chain.create",
            ACTOR,
            None,
            true,
            false,
            false,
            json!({"creators":[]}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::OK);
    let created: Value = create.json().await.unwrap();
    let chain_id = created["result"]["chain_id"].as_str().unwrap();
    let revoke = post(
        &client,
        &url,
        &envelope(
            "security.revoke",
            ACTOR,
            Some(chain_id),
            true,
            true,
            true,
            json!({"source_id":"source-never-created"}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(revoke.status(), reqwest::StatusCode::FORBIDDEN);
    let value: Value = revoke.json().await.unwrap();
    assert_eq!(value["error"]["code"], "revoke_denied");
    task.abort();
}

#[tokio::test]
async fn valid_create_query_and_unknown_operation_use_safe_statuses() {
    let (url, task) = start(core().await, auth(true, false, false)).await;
    let client = loopback_client();
    let health = client
        .get(format!("{url}/health"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    assert_eq!(health.headers()[reqwest::header::CACHE_CONTROL], "no-store");
    let caps = client
        .get(format!("{url}/api/v2/capabilities"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(caps.status(), reqwest::StatusCode::OK);
    let cap_value: Value = caps.json().await.unwrap();
    assert_eq!(cap_value["implementation"], "sic-rust-core-development");
    assert!(!cap_value.to_string().contains(TOKEN));

    let create = post(
        &client,
        &url,
        &envelope(
            "chain.create",
            ACTOR,
            None,
            true,
            false,
            false,
            json!({"creators":[]}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::OK);
    let created: Value = create.json().await.unwrap();
    let chain_id = created["result"]["chain_id"].as_str().unwrap().to_owned();

    let get = post(
        &client,
        &url,
        &envelope(
            "chain.get",
            ACTOR,
            Some(&chain_id),
            false,
            false,
            false,
            json!({}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(get.status(), reqwest::StatusCode::OK);
    let got: Value = get.json().await.unwrap();
    assert_eq!(got["result"]["chain_id"], chain_id);

    let unknown = post(
        &client,
        &url,
        &envelope(
            "future.operation",
            ACTOR,
            None,
            false,
            false,
            false,
            json!({}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(unknown.status(), reqwest::StatusCode::NOT_IMPLEMENTED);
    let unknown_body = unknown.text().await.unwrap();
    assert!(!unknown_body.contains(TOKEN));
    task.abort();
}

#[tokio::test]
async fn unknown_envelope_fields_are_rejected_without_reflecting_body_or_token() {
    let (url, task) = start(core().await, auth(true, true, true)).await;
    let client = loopback_client();
    let body = json!({
        "operation":"chain.create",
        "context":{"actor":ACTOR,"chain_id":null,"allow_write":true,"allow_model":false,"allow_revoke":false,"admin_token":"private-field"},
        "payload":{"creators":[]}
    });
    let response = post(&client, &url, &body, Some(TOKEN)).await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let text = response.text().await.unwrap();
    assert!(!text.contains("private-field"));
    assert!(!text.contains(TOKEN));
    task.abort();
}

#[tokio::test]
async fn request_body_is_limited_to_sixteen_mib() {
    let (url, task) = start(core().await, auth(true, true, true)).await;
    let client = loopback_client();
    let body = " ".repeat(MAX_REQUEST_BODY_BYTES + 1);
    let response = client
        .post(format!("{url}/api/v2/operations"))
        .bearer_auth(TOKEN)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers()[reqwest::header::CACHE_CONTROL],
        "no-store"
    );
    task.abort();
}
