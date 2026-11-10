//! Authenticated HTTP control plane for explicitly implemented Rust Core APIs.
//!
//! This module constructs a router only. The host process owns binding, loopback
//! policy, token-file handling, and shutdown.

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

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
}

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
        })
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
    Router::new()
        .route("/health", get(health))
        .route("/api/v2/capabilities", get(capabilities))
        .route("/api/v2/operations", post(operations))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state, authenticate))
        // This outer layer also adds no-store to authentication and body-limit
        // rejections that return before a handler runs.
        .layer(middleware::from_fn(no_store))
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
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(candidate) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(candidate.as_bytes(), expected.as_bytes())
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
