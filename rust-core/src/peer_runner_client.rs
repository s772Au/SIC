//! Host-side, loopback-only client for the authenticated SIC Core control API.
//!
//! This is an IPC client for a trusted local supervisor. It has no retry
//! policy and never forwards its bearer to a proxy or redirect target.

use reqwest::{Client, Url, header::AUTHORIZATION};
use serde_json::{Value, json};
use std::{fmt, net::IpAddr, time::Duration};

const OPERATIONS_PATH: &str = "/api/v2/operations";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
pub struct HostPermissions {
    allow_write: bool,
    allow_model: bool,
    allow_revoke: bool,
}

impl HostPermissions {
    pub fn new(allow_write: bool, allow_model: bool, allow_revoke: bool) -> Self {
        Self {
            allow_write,
            allow_model,
            allow_revoke,
        }
    }
}

/// No `Debug` or serde implementation: the bearer stays in this process only.
pub struct PeerRunnerClient {
    client: Client,
    endpoint: Url,
    bearer_token: String,
    actor: String,
    permissions: HostPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    InvalidConfiguration,
    InvalidOperation,
    RequestFailed,
    HttpStatus(u16),
    InvalidResponse,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => f.write_str("invalid local Core client configuration"),
            Self::InvalidOperation => f.write_str("invalid Core operation request"),
            Self::RequestFailed => f.write_str("local Core request failed"),
            Self::HttpStatus(status) => write!(f, "local Core returned HTTP status {status}"),
            Self::InvalidResponse => f.write_str("local Core returned an invalid response"),
        }
    }
}

impl std::error::Error for ClientError {}

impl PeerRunnerClient {
    /// Construct a client for a literal IPv4/IPv6 loopback URL only.
    ///
    /// The URL must be plain HTTP with a root path and no credentials,
    /// query, or fragment. Proxy use and redirects are disabled in reqwest.
    pub fn new(
        base_url: &str,
        bearer_token: impl Into<String>,
        actor: impl Into<String>,
        permissions: HostPermissions,
    ) -> Result<Self, ClientError> {
        Self::new_with_timeout(
            base_url,
            bearer_token,
            actor,
            permissions,
            Duration::from_secs(30),
        )
    }

    /// Model-backed host operations may legitimately take longer than a local
    /// metadata read. The host sets a finite deadline; timeouts are not retried.
    pub fn new_with_timeout(
        base_url: &str,
        bearer_token: impl Into<String>,
        actor: impl Into<String>,
        permissions: HostPermissions,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let mut endpoint = Url::parse(base_url).map_err(|_| ClientError::InvalidConfiguration)?;
        let token = bearer_token.into();
        let actor = actor.into();
        let authority = base_url
            .split_once("//")
            .map(|(_, remainder)| remainder.split(['/', '?', '#']).next().unwrap_or_default());
        let host_is_loopback = endpoint
            .host_str()
            .and_then(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback());
        if endpoint.scheme() != "http"
            || timeout.is_zero()
            || timeout > Duration::from_secs(3600)
            || !host_is_loopback
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || authority.is_some_and(|value| value.contains('@'))
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
            || token.is_empty()
            || token.trim() != token
            || token.len() > 4096
            || token.chars().any(char::is_control)
            || actor.trim().is_empty()
            || actor.len() > 256
            || actor.chars().any(char::is_control)
        {
            return Err(ClientError::InvalidConfiguration);
        }
        endpoint.set_path(OPERATIONS_PATH);
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .build()
            .map_err(|_| ClientError::InvalidConfiguration)?;
        Ok(Self {
            client,
            endpoint,
            bearer_token: token,
            actor,
            permissions,
        })
    }

    /// Call the existing `/api/v2/operations` envelope. Permissions always
    /// come from this host-owned client configuration, never from the caller.
    pub async fn operate(
        &self,
        operation: &str,
        chain_id: Option<&str>,
        payload: &Value,
    ) -> Result<Value, ClientError> {
        if operation.trim().is_empty()
            || operation.len() > 256
            || operation.chars().any(char::is_control)
            || chain_id.is_some_and(|chain| {
                chain.trim().is_empty() || chain.len() > 256 || chain.chars().any(char::is_control)
            })
        {
            return Err(ClientError::InvalidOperation);
        }
        let bearer = format!("Bearer {}", self.bearer_token);
        let authorization = reqwest::header::HeaderValue::from_str(&bearer)
            .map_err(|_| ClientError::InvalidConfiguration)?;
        let body = json!({
            "operation":operation,
            "context":{
                "actor":self.actor,
                "chain_id":chain_id,
                "allow_write":self.permissions.allow_write,
                "allow_model":self.permissions.allow_model,
                "allow_revoke":self.permissions.allow_revoke
            },
            "payload":payload
        });
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, authorization)
            .json(&body)
            .send()
            .await
            .map_err(|_| ClientError::RequestFailed)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::HttpStatus(status.as_u16()));
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ClientError::InvalidResponse)?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(ClientError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| ClientError::InvalidResponse)
    }
}
