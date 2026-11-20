//! Authenticated HTTP control plane for explicitly implemented Rust Core APIs.
//!
//! This module constructs a router only. The host process owns binding, loopback
//! policy, token-file handling, and shutdown.

use axum::{
    Extension, Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State, rejection::{BytesRejection, JsonRejection}},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::{Core, OperationContext};

pub const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Explicit host-authenticated identity and permission ceiling.
///
/// The token is intentionally neither `Debug` nor serializable. This config is
/// passed by the host and is not read from request content.
#[derive(Clone)]
pub struct HostAuth {
    bearer_token: String,
    actor: String,
    allow_write: bool,
    allow_model: bool,
    allow_revoke: bool,
    archive_grants: Vec<ChainGrant>,
    harness_grants: Vec<ChainGrant>,
}

#[derive(Clone)]
struct ChainGrant {
    digest: [u8; 32],
    chain_id: String,
}

#[derive(Clone)]
struct ChainScope(String);

impl HostAuth {
    pub fn new(
        bearer_token: impl Into<String>,
        actor: impl Into<String>,
        allow_write: bool,
        allow_model: bool,
        allow_revoke: bool,
    ) -> Result<Self, String> {
        let bearer_token = bearer_token.into();
        let actor = actor.into();
        if bearer_token.is_empty() || bearer_token.trim() != bearer_token {
            return Err("Bearer token must be nonempty without surrounding whitespace".to_owned());
        }
        if actor.trim().is_empty() {
            return Err("host actor must be nonempty".to_owned());
        }
        Ok(Self {
            bearer_token,
            actor,
            allow_write,
            allow_model,
            allow_revoke,
            archive_grants: Vec::new(),
            harness_grants: Vec::new(),
        })
    }

    pub fn with_archive_grant(mut self, bearer: &str, chain_id: String) -> Result<Self, String> {
        let grant = self.chain_grant(bearer, chain_id)?;
        self.archive_grants.push(grant);
        Ok(self)
    }

    /// Delegate the native editing lifecycle to a trusted host for one chain.
    /// Model tools must use a separate archive credential.
    pub fn with_harness_grant(mut self, bearer: &str, chain_id: String) -> Result<Self, String> {
        let grant = self.chain_grant(bearer, chain_id)?;
        self.harness_grants.push(grant);
        Ok(self)
    }

    fn chain_grant(&self, bearer: &str, chain_id: String) -> Result<ChainGrant, String> {
        if bearer.len() < 32 || !bearer.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("Scoped bearer requires at least 32 visible ASCII characters".into());
        }
        if chain_id.is_empty() || chain_id.len() > 256
            || chain_id.trim() != chain_id || chain_id.chars().any(char::is_control) {
            return Err("Scoped grant requires a bounded chain ID".into());
        }
        let digest: [u8; 32] = Sha256::digest(bearer).into();
        if constant_time_eq(bearer.as_bytes(), self.bearer_token.as_bytes())
            || self.archive_grants.iter().chain(&self.harness_grants)
                .any(|grant| constant_time_eq(&digest, &grant.digest)) {
            return Err("Scoped bearer must be distinct from every configured credential".into());
        }
        Ok(ChainGrant { digest, chain_id })
    }
}

#[derive(Clone)]
struct AppState {
    core: Core,
    auth: HostAuth,
}

/// Construct API routes without binding a socket or choosing a public address.
pub fn router(core: Core, auth: HostAuth) -> Router {
    let state = AppState { core, auth };
    let host = Router::new()
        .route("/health", get(health))
        .route("/api/v2/capabilities", get(capabilities))
        .route("/api/v2/operations", post(operations))
        .route("/api/v2/harness/turn", post(harness_turn))
        .route("/api/v2/chains/{chain_id}/ledger", get(ledger))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate));
    let archive = Router::new()
        .route("/api/v2/archive/operations", post(archive_operation))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(state.auth.archive_grants.clone(), authenticate_chain));
    let harness = Router::new()
        .route("/api/v2/harness/operations", post(harness_operation))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.auth.harness_grants, authenticate_chain));
    host.merge(archive).merge(harness)
        // This outer layer also adds no-store to authentication and body-limit
        // rejections that return before a handler runs.
        .layer(middleware::from_fn(no_store))
}

async fn authenticate_chain(
    State(grants): State<Vec<ChainGrant>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let grant = bearer_value(request.headers()).and_then(|token| {
        let digest = Sha256::digest(token);
        grants.iter().find(|grant| constant_time_eq(&digest, &grant.digest))
    });
    let Some(grant) = grant else {
        return error_response(StatusCode::UNAUTHORIZED, "unauthorized", "Scoped chain authentication required");
    };
    request.extensions_mut().insert(ChainScope(grant.chain_id.clone()));
    next.run(request).await
}

/// Add the DSH protocol under the same host authentication and permission
/// ceiling. The actor is derived from the bearer, never from DSH JSON.
pub fn router_with_dsh(
    core: Core,
    auth: HostAuth,
    bridge: std::sync::Arc<crate::dsh_bridge::DshBridge>,
) -> Router {
    let guard = auth.clone();
    let resolver: crate::dsh_bridge::ActorResolver = std::sync::Arc::new(move |headers| {
        if guard.allow_write && guard.allow_model && valid_bearer(headers, &guard.bearer_token) {
            Ok(guard.actor.clone())
        } else {
            Err("host authentication or model grant required".into())
        }
    });
    router(core, auth).nest("/v1", crate::dsh_bridge::router(bridge, resolver))
}

async fn authenticate(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if valid_bearer(request.headers(), &state.auth.bearer_token) {
        next.run(request).await
    } else {
        error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Bearer authentication required",
        )
    }
}

fn valid_bearer(headers: &HeaderMap, expected: &str) -> bool {
    bearer_value(headers).is_some_and(|candidate| constant_time_eq(candidate.as_bytes(), expected.as_bytes()))
}

fn bearer_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..max_len {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(a ^ b);
    }
    difference == 0
}

async fn no_store(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn health() -> Response {
    (
        StatusCode::OK,
        Json(json!({"ok":true,"service":"sic-rust-core"})),
    )
        .into_response()
}

async fn capabilities(State(_state): State<AppState>) -> Response {
    (StatusCode::OK, Json(Core::capabilities())).into_response()
}

/// Trusted host control plane, deliberately absent from Core operations and
/// model tool registries. The authenticated Harness supplies actual session,
/// turn and human-source provenance; native tool arguments cannot mint grants.
async fn harness_turn(State(state):State<AppState>, Json(grant):Json<crate::harness_lifecycle::HarnessGrant>) -> Response {
    let context=OperationContext {actor:state.auth.actor.clone(),allow_write:state.auth.allow_write,
        allow_model:false,allow_revoke:false,chain_id:None};
    match state.core.issue_harness_grant(&context,grant).await {
        Ok(value)=>(StatusCode::OK,Json(value)).into_response(),
        Err(error)=>core_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextInput {
    actor: String,
    #[serde(default)]
    chain_id: Option<String>,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    allow_model: bool,
    #[serde(default)]
    allow_revoke: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    operation: String,
    context: ContextInput,
    payload: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopedOperation {
    operation: String,
    payload: Value,
}

async fn archive_operation(
    State(state): State<AppState>,
    Extension(scope): Extension<ChainScope>,
    request: Result<Json<ScopedOperation>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return invalid_scoped_request(error),
    };
    if !matches!(request.operation.as_str(), "memory.archive_search" | "memory.source_text_page") {
        return error_response(StatusCode::FORBIDDEN, "archive_operation_denied", "Only archive search and source pages are available");
    }
    let context = OperationContext {
        actor: state.auth.actor.clone(),
        chain_id: Some(scope.0),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    match state.core.operate(&request.operation, &context, &request.payload).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => core_error_response(error),
    }
}

async fn harness_operation(
    State(state): State<AppState>,
    Extension(scope): Extension<ChainScope>,
    request: Result<Json<ScopedOperation>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return invalid_scoped_request(error),
    };
    let permitted = match request.operation.as_str() {
        "cut.get" | "chain.get" | "native.begin" | "native.get" | "native.history" |
        "native.preview" | "native.commit" | "native.publish" | "native.abandon" |
        "edit.host.get" | "edit.host.claim" | "edit.host.submit" | "edit.host.fail" => true,
        "hierarchy.expand" => request.payload.as_object().is_some_and(|payload| {
            payload.len() == 1 && payload.get("cb_id").is_some_and(Value::is_string)
        }),
        "runtime.schedule" => request.payload["context_id"] == "main"
            && request.payload["editor_mode"] == "host"
            && request.payload["max_model_calls"].as_u64() == Some(0)
            && request.payload["program_rules"].as_array().is_some_and(Vec::is_empty),
        _ => false,
    };
    if !permitted {
        return error_response(StatusCode::FORBIDDEN, "harness_operation_denied",
            "Operation is outside the native host editing scope");
    }
    let context = OperationContext {
        actor: state.auth.actor.clone(),
        chain_id: Some(scope.0),
        allow_write: state.auth.allow_write,
        allow_model: false,
        allow_revoke: false,
    };
    match state.core.operate(&request.operation, &context, &request.payload).await {
        Ok(mut value) if request.operation == "hierarchy.expand" => {
            let result = &value["result"];
            value["result"] = json!({"cb_id": result["cb_id"], "level": result["level"], "members": result["members"]});
            (StatusCode::OK, Json(value)).into_response()
        }
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => core_error_response(error),
    }
}

fn invalid_scoped_request(error: JsonRejection) -> Response {
    error_response(error.status(), "invalid_scoped_request", "Scoped operation request is invalid")
}

/// Read-only host-scoped ledger: the authenticated actor may read accounting
/// for a chain it owns. The bearer is never accepted as a chain identity.
async fn ledger(State(state): State<AppState>, Path(chain_id): Path<String>) -> Response {
    if chain_id.is_empty() || chain_id.len() > 256 || chain_id.chars().any(char::is_control) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_chain",
            "chain_id must be bounded text",
        );
    }
    let context = OperationContext {
        actor: state.auth.actor.clone(),
        chain_id: Some(chain_id.clone()),
        allow_write: false,
        allow_model: false,
        allow_revoke: false,
    };
    match state
        .core
        .operate("accounting.get", &context, &json!({}))
        .await
    {
        Ok(value) => (
            StatusCode::OK,
            Json(json!({"chain_id":chain_id,"accounting":value["result"].clone()})),
        )
            .into_response(),
        Err(error) => core_error_response(error),
    }
}

async fn operations(
    State(state): State<AppState>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(rejection) => {
            let status = rejection.into_response().status();
            if status == StatusCode::PAYLOAD_TOO_LARGE {
                return error_response(status, "payload_too_large", "Request body exceeds 16 MiB");
            }
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                "Request body is invalid",
            );
        }
    };
    let request: OperationRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Request must match the operation envelope",
            );
        }
    };
    if request.operation.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Operation name is required",
        );
    }
    if request.context.actor != state.auth.actor {
        return error_response(
            StatusCode::FORBIDDEN,
            "actor_mismatch",
            "Request actor is not authorized by this host",
        );
    }
    let context = OperationContext {
        actor: state.auth.actor.clone(),
        chain_id: request.context.chain_id,
        allow_write: state.auth.allow_write && request.context.allow_write,
        allow_model: state.auth.allow_model && request.context.allow_model,
        allow_revoke: state.auth.allow_revoke && request.context.allow_revoke,
    };
    match state
        .core
        .operate(&request.operation, &context, &request.payload)
        .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => core_error_response(error),
    }
}

fn core_error_response(error: crate::error::CoreError) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::NOT_IMPLEMENTED {
        return error_response(
            status,
            "unsupported_operation",
            "Operation is not implemented",
        );
    }
    if status.is_server_error() {
        error_response(status, "internal_error", "Operation failed")
    } else {
        error_response(status, error.code, &error.message)
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error":{"code":code,"message":message}})),
    )
        .into_response()
}
