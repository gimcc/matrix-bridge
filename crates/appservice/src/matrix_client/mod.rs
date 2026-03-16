mod crypto;
mod media;
mod messaging;
mod puppets;
mod rooms;

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use secrecy::SecretString;
use tracing::warn;

/// HTTP client wrapper for making authenticated requests to the Matrix homeserver
/// using the appservice `as_token` for identity assertion.
#[derive(Clone)]
pub struct MatrixClient {
    pub(crate) client: Client,
    pub(crate) homeserver_url: String,
    pub(crate) as_token: Arc<SecretString>,
    pub(crate) server_name: String,
    /// Device ID for MSC3202 appservice device masquerading.
    /// When set, E2EE key management requests include `device_id` and `user_id` in query params.
    pub(crate) device_id: Option<String>,
    /// Bot user ID for MSC3202. Must be set alongside device_id to avoid
    /// Synapse passing a UserID object (instead of str) to DB queries.
    pub(crate) bot_user_id: Option<String>,
}

impl MatrixClient {
    pub fn new(homeserver_url: &str, as_token: &str, server_name: &str) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            homeserver_url: homeserver_url.trim_end_matches('/').to_string(),
            as_token: Arc::new(SecretString::from(as_token.to_string())),
            server_name: server_name.to_string(),
            device_id: None,
            bot_user_id: None,
        })
    }

    /// Set the device ID and bot user ID for MSC3202 device masquerading.
    /// Both must be set so that Synapse receives `user_id` and `device_id`
    /// query params, avoiding an internal bug where `app_service.sender`
    /// (a UserID object) is passed directly to DB queries.
    pub fn set_device_id(&mut self, device_id: &str, sender_localpart: &str) {
        self.device_id = Some(device_id.to_string());
        self.bot_user_id = Some(format!("@{}:{}", sender_localpart, self.server_name));
    }

    /// Create a clone of this MatrixClient that acts on behalf of a specific puppet user/device.
    /// E2EE endpoints (keys/upload, keys/query, keys/claim, sendToDevice) will carry
    /// the puppet's `user_id` and `device_id` instead of the bridge bot's.
    pub fn with_user_device(&self, user_id: &str, device_id: &str) -> Self {
        Self {
            client: self.client.clone(),
            homeserver_url: self.homeserver_url.clone(),
            as_token: Arc::clone(&self.as_token),
            server_name: self.server_name.clone(),
            device_id: Some(device_id.to_string()),
            bot_user_id: Some(user_id.to_string()),
        }
    }

    /// Get the server name.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Convert a reqwest Response into an http::Response<Vec<u8>> for ruma deserialization.
    ///
    /// Logs a warning for non-2xx responses to aid E2EE debugging.
    pub(crate) async fn to_http_response(
        resp: reqwest::Response,
    ) -> anyhow::Result<http::Response<Vec<u8>>> {
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let body_preview = String::from_utf8_lossy(&bytes);
            warn!(
                status = %status,
                body = %body_preview,
                "homeserver returned non-2xx for crypto request"
            );
        }
        let http_resp = http::Response::builder()
            .status(status)
            .body(bytes.to_vec())?;
        Ok(http_resp)
    }

    /// Extract `event_id` from a successful Matrix send response.
    pub(crate) async fn extract_event_id(resp: reqwest::Response) -> anyhow::Result<String> {
        let body: serde_json::Value = resp.json().await?;
        body.get("event_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("response missing event_id"))
    }

    /// Check an HTTP response status and return an error with context if non-2xx.
    ///
    /// Use this for endpoints where failure should propagate (most operations).
    /// For non-critical endpoints (e.g. profile updates), handle errors inline.
    pub(crate) async fn check_response(
        resp: reqwest::Response,
        operation: &str,
    ) -> anyhow::Result<reqwest::Response> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status();
            let text = resp.text().await?;
            anyhow::bail!("{operation} failed ({status}): {text}");
        }
    }

    /// Build query params for E2EE requests (MSC3202 device masquerading).
    pub(crate) fn e2ee_query_params(&self) -> Vec<(&str, String)> {
        let mut params = Vec::new();
        if let Some(ref uid) = self.bot_user_id {
            params.push(("user_id", uid.clone()));
        }
        if let Some(ref did) = self.device_id {
            params.push(("device_id", did.clone()));
        }
        params
    }
}
