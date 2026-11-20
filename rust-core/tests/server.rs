pub mod core {
    pub use sic_core_rust::core::*;
}
pub mod error {
    pub use sic_core_rust::error::*;
}
pub mod dsh_bridge {
    pub use sic_core_rust::dsh_bridge::*;
}
pub mod harness_lifecycle {
    pub use sic_core_rust::harness_lifecycle::*;
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
async fn native_host_grants_pin_the_chain_and_exclude_administration_and_model_execution() {
    const HOST: &str = "synthetic-native-host-grant-00000001";
    const ARCHIVE: &str = "synthetic-archive-only-grant-00000001";
    let core = core().await;
    let mut owner = core::OperationContext {
        actor: ACTOR.into(), allow_write: true, ..Default::default()
    };
    let chain = core.operate("chain.create", &owner, &json!({})).await.unwrap()
        ["result"]["chain_id"].as_str().unwrap().to_owned();
    let other = core.operate("chain.create", &owner, &json!({})).await.unwrap()
        ["result"]["chain_id"].as_str().unwrap().to_owned();
    owner.chain_id = Some(other.clone());
    let foreign_source = core.operate("native.begin", &owner, &json!({"format":"pi-messages",
        "request":{"messages":[{"role":"user","content":"different chain"}]}
    })).await.unwrap()["result"]["source_id"].clone();
    let auth = auth(true, true, true).with_harness_grant(HOST, chain.clone()).unwrap();
    assert!(auth.clone().with_archive_grant(HOST, other.clone()).is_err());
    assert!(auth.clone().with_harness_grant(HOST, other.clone()).is_err());
    let auth = auth.with_archive_grant(ARCHIVE, chain.clone()).unwrap();
    assert!(auth.clone().with_harness_grant(ARCHIVE, other.clone()).is_err());
    let (url, task) = start(core, auth).await;
    let client = loopback_client();
    let endpoint = format!("{url}/api/v2/harness/operations");
    let send = |operation: &str, payload: Value, token: &str| client.post(&endpoint)
        .bearer_auth(token).json(&json!({"operation":operation,"payload":payload})).send();

    for token in [ARCHIVE, TOKEN, "incorrect-scope"] {
        assert_eq!(send("cut.get", json!({}), token).await.unwrap().status(), 401);
    }
    let begun: Value = send("native.begin", json!({"format":"pi-messages","request":{
        "messages":[{"role":"user","content":"authorized raw user"}]}}), HOST).await.unwrap()
        .error_for_status().unwrap().json().await.unwrap();
    assert_eq!(begun["context"]["chain_id"], chain);
    let source = &begun["result"];
    let identity = json!({"source_id":source["source_id"],"expected_source_digest":source["source_digest"]});
    let mut commit = identity.clone();
    commit["expected_version"] = json!(0);
    commit["edits"] = json!([{"unit_id":"u1","expected_text":"authorized raw user","replacement":"short"}]);
    assert_eq!(send("native.commit", commit, HOST).await.unwrap().status(), 200);
    let mut publish = identity;
    publish["expected_version"] = json!(1);
    publish["expected_cut"] = json!(0);
    publish["idempotency_key"] = json!("scoped-host-publish");
    publish["ops"] = json!([{"op":"COMPRESS","target":"u1","content":"short"}]);
    publish["pvi"] = json!({});
    let published: Value = send("native.publish", publish, HOST).await.unwrap().error_for_status()
        .unwrap().json().await.unwrap();
    let cb = &published["result"]["publication"]["publication"]["cb_id"];
    assert!(cb.is_string());
    let lineage: Value = send("hierarchy.expand", json!({"cb_id":cb}), HOST).await.unwrap().error_for_status()
        .unwrap().json().await.unwrap();
    assert_eq!(lineage["result"], json!({"cb_id":cb,"level":1,"members":[]}));
    assert!(!lineage.to_string().contains("authorized raw user"));
    assert_eq!(send("hierarchy.expand", json!({"wb_id":"unbound"}), HOST).await.unwrap().status(), 403);
    let denied = send("native.get", json!({"source_id":foreign_source}), HOST).await.unwrap();
    assert_eq!(denied.status(), 403);
    assert!(!denied.text().await.unwrap().contains("different chain"));
    for operation in ["chain.create", "worker.run", "security.revoke", "edit.host.reopen", "memory.source_text_page"] {
        assert_eq!(send(operation, json!({}), HOST).await.unwrap().status(), 403, "{operation}");
    }
    let forbidden_schedule = json!({"context_id":"main","editor_mode":"program_fixture","max_model_calls":0,"program_rules":[]});
    assert_eq!(send("runtime.schedule", forbidden_schedule, HOST).await.unwrap().status(), 403);
    let forged = envelope("cut.get", ACTOR, Some(&other), true, true, true, json!({}));
    let response = client.post(&endpoint).bearer_auth(HOST).json(&forged).send().await.unwrap();
    assert_eq!(response.status(), 422);
    assert_eq!(post(&client, &url, &forged, Some(HOST)).await.status(), 401);
    let archive_endpoint = format!("{url}/api/v2/archive/operations");
    let response = client.post(&archive_endpoint).bearer_auth(HOST)
        .json(&json!({"operation":"memory.archive_search","payload":{"query":"raw"}})).send().await.unwrap();
    assert_eq!(response.status(), 401);
    for (path, token) in [(&endpoint, HOST), (&archive_endpoint, ARCHIVE)] {
        let bad = client.post(path).bearer_auth(token)
            .json(&json!({"operation":"cut.get","payload":{},"private-witness-field":true})).send().await.unwrap();
        assert_eq!(bad.status(), 422);
        assert_eq!(bad.headers()[reqwest::header::CACHE_CONTROL], "no-store");
        assert!(!bad.text().await.unwrap().contains("private-witness-field"));
    }
    task.abort();
}

#[tokio::test]
async fn archive_credentials_authorize_only_published_reads_on_the_configured_chain() {
    const ARCHIVE: &str = "synthetic-archive-only-credential-00001";
    const OTHER: &str = "synthetic-archive-only-credential-00002";
    let core = core().await;
    async fn publish(core: &Core, text: &str) -> (String, Value) {
        let mut context = core::OperationContext {
            actor: ACTOR.into(), chain_id: None, allow_write: true, allow_model: false, allow_revoke: false,
        };
        let created = core.operate("chain.create", &context, &json!({})).await.unwrap();
        let chain = created["result"]["chain_id"].as_str().unwrap().to_owned();
        context.chain_id = Some(chain.clone());
        let started = core.operate("native.begin", &context, &json!({"format":"openai-responses",
            "editable_unit_ids":["u1"],"request":{"model":"fixture","input":[{"role":"user",
                "content":[{"type":"input_text","text":text}]}]}})).await.unwrap();
        let source = &started["result"];
        core.operate("native.commit", &context, &json!({"source_id":source["source_id"],
            "expected_source_digest":source["source_digest"],"expected_version":0,
            "edits":[{"unit_id":"u1","expected_text":text,"replacement":"note"}]})).await.unwrap();
        core.operate("native.publish", &context, &json!({"source_id":source["source_id"],
            "expected_source_digest":source["source_digest"],"expected_version":1,"expected_cut":0,
            "idempotency_key":"fixture","ops":[{"op":"COMPRESS","target":"u1","content":"note"}],
            "pvi":{}})).await.unwrap();
        (chain, source["source_id"].clone())
    }
    let (chain, source) = publish(&core, "bound archive original").await;
    let (other_chain, other_source) = publish(&core, "another chain original").await;
    assert!(HostAuth::new(ARCHIVE, ACTOR, true, false, false).unwrap()
        .with_archive_grant(ARCHIVE, chain.clone()).is_err());
    let auth = auth(true, true, true).with_archive_grant(ARCHIVE, chain.clone()).unwrap();
    assert!(auth.clone().with_archive_grant(ARCHIVE, other_chain.clone()).is_err());
    let (url, task) = start(core, auth.with_archive_grant(OTHER, other_chain.clone()).unwrap()).await;
    let client = loopback_client();
    let endpoint = format!("{url}/api/v2/archive/operations");
    let search = json!({"operation":"memory.archive_search","payload":{"query":"bound archive"}});
    let response = client.post(&endpoint).bearer_auth(ARCHIVE).json(&search).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()[reqwest::header::CACHE_CONTROL], "no-store");
    let hits: Value = response.json().await.unwrap();
    assert_eq!(hits["result"]["hits"].as_array().unwrap().len(), 1);

    let page = |id| json!({"operation":"memory.source_text_page","payload":{"source_id":id,"unit_id":"u1"}});
    let response: Value = client.post(&endpoint).bearer_auth(ARCHIVE).json(&page(source))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(response["result"]["text"], "bound archive original");
    let forbidden = client.post(&endpoint).bearer_auth(ARCHIVE).json(&page(other_source)).send().await.unwrap();
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);
    let response: Value = client.post(&endpoint).bearer_auth(OTHER).json(&search)
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(response["result"]["hits"].as_array().unwrap().len(), 0);

    for op in ["chain.create", "native.publish", "harness.call", "accounting.get", "worker.run", "security.revoke"] {
        let denied = client.post(&endpoint).bearer_auth(ARCHIVE)
            .json(&json!({"operation":op,"payload":{}})).send().await.unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
    }
    let forged = envelope("memory.archive_search", ACTOR, Some(&other_chain), true, true, true, json!({"query":"original"}));
    let invalid = client.post(&endpoint).bearer_auth(ARCHIVE).json(&forged).send().await.unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(post(&client, &url, &forged, Some(ARCHIVE)).await.status(), reqwest::StatusCode::UNAUTHORIZED);
    let read_only = client.post(&endpoint).bearer_auth(TOKEN).json(&search).send().await.unwrap();
    assert_eq!(read_only.status(), reqwest::StatusCode::UNAUTHORIZED);
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

#[tokio::test]
async fn ledger_route_requires_bearer_and_returns_host_scoped_accounting() {
    let (url, task) = start(core().await, auth(true, false, false)).await;
    let client = loopback_client();
    let created = post(
        &client,
        &url,
        &envelope("chain.create", ACTOR, None, true, false, false, json!({})),
        Some(TOKEN),
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::OK);
    let body: Value = created.json().await.unwrap();
    let chain_id = body["result"]["chain_id"].as_str().unwrap().to_owned();

    let unauthorized = client
        .get(format!("{url}/api/v2/chains/{chain_id}/ledger"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);

    let ledger = client
        .get(format!("{url}/api/v2/chains/{chain_id}/ledger"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(ledger.status(), reqwest::StatusCode::OK);
    assert_eq!(ledger.headers()[reqwest::header::CACHE_CONTROL], "no-store");
    let ledger: Value = ledger.json().await.unwrap();
    assert_eq!(ledger["chain_id"], chain_id);
    assert_eq!(
        ledger["accounting"]["resources"]["storage"]["class"],
        "host_measured"
    );
    assert!(ledger["accounting"]["measurements"]["blocks"].is_object());
    assert!(!ledger.to_string().contains(TOKEN));

    let missing = client
        .get(format!("{url}/api/v2/chains/chain_missing/ledger"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_ne!(missing.status(), reqwest::StatusCode::OK);
    task.abort();
}

#[tokio::test]
async fn ledger_route_does_not_leak_revoked_source_content() {
    const SECRET: &str = "LEDGER-REVOKED-SECRET";
    let (url, task) = start(core().await, auth(true, false, true)).await;
    let client = loopback_client();
    let created: Value = post(
        &client,
        &url,
        &envelope("chain.create", ACTOR, None, true, false, false, json!({})),
        Some(TOKEN),
    )
    .await
    .json()
    .await
    .unwrap();
    let chain_id = created["result"]["chain_id"].as_str().unwrap().to_owned();
    let ingested: Value = post(
        &client,
        &url,
        &envelope(
            "source.ingest",
            ACTOR,
            Some(&chain_id),
            true,
            false,
            false,
            json!({"request":{"messages":[{"role":"user","content":SECRET}]}}),
        ),
        Some(TOKEN),
    )
    .await
    .json()
    .await
    .unwrap();
    let source_id = ingested["result"]["source_id"].as_str().unwrap().to_owned();
    let revoked = post(
        &client,
        &url,
        &envelope(
            "security.revoke",
            ACTOR,
            Some(&chain_id),
            true,
            false,
            true,
            json!({"source_id":source_id,"reason":"ledger test"}),
        ),
        Some(TOKEN),
    )
    .await;
    assert_eq!(revoked.status(), reqwest::StatusCode::OK);

    let ledger = client
        .get(format!("{url}/api/v2/chains/{chain_id}/ledger"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(ledger.status(), reqwest::StatusCode::OK);
    let text = ledger.text().await.unwrap();
    assert!(!text.contains(SECRET), "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        body["accounting"]["measurements"]["suppressed"],
        "revocation"
    );
    task.abort();
}
