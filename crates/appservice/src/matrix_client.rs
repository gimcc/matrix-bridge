use std::time::Duration;

use reqwest::Client;
use ruma::api::IncomingResponse;
use ruma::api::client::{
    keys::{
        claim_keys::v3::Response as KeysClaimResponse, get_keys::v3::Response as KeysQueryResponse,
        upload_keys::v3::Response as KeysUploadResponse,
    },
    to_device::send_event_to_device::v3::Response as ToDeviceResponse,
};
use serde_json::{Value, json};
use tracing::{debug, warn};

use matrix_sdk_crypto::types::requests::{KeysQueryRequest, ToDeviceRequest};
use ruma::api::client::keys::{
    claim_keys::v3::Request as KeysClaimRequest, upload_keys::v3::Request as KeysUploadRequest,
};

/// HTTP client wrapper for making authenticated requests to the Matrix homeserver
/// using the appservice `as_token` for identity assertion.
#[derive(Clone)]
pub struct MatrixClient {
    client: Client,
    homeserver_url: String,
    as_token: String,
    server_name: String,
    /// Device ID for MSC3202 appservice device masquerading.
    /// When set, E2EE key management requests include `device_id` and `user_id` in query params.
    device_id: Option<String>,
    /// Bot user ID for MSC3202. Must be set alongside device_id to avoid
    /// Synapse passing a UserID object (instead of str) to DB queries.
    bot_user_id: Option<String>,
}

impl MatrixClient {
    pub fn new(homeserver_url: &str, as_token: &str, server_name: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");

        Self {
            client,
            homeserver_url: homeserver_url.trim_end_matches('/').to_string(),
            as_token: as_token.to_string(),
            server_name: server_name.to_string(),
            device_id: None,
            bot_user_id: None,
        }
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
            as_token: self.as_token.clone(),
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
    async fn to_http_response(resp: reqwest::Response) -> anyhow::Result<http::Response<Vec<u8>>> {
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

    /// Register a puppet user via the appservice API.
    /// Uses `POST /_matrix/client/v3/register` with `type: m.login.application_service`.
    pub async fn register_puppet(&self, localpart: &str) -> anyhow::Result<String> {
        self.register_puppet_with_device(localpart, None).await
    }

    /// Register a puppet user, optionally creating a specific device.
    /// When `device_id` is provided, Synapse creates the device on the user.
    pub async fn register_puppet_with_device(
        &self,
        localpart: &str,
        device_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let user_id = format!("@{}:{}", localpart, self.server_name);
        let url = format!("{}/_matrix/client/v3/register", self.homeserver_url);

        let mut body = json!({
            "type": "m.login.application_service",
            "username": localpart,
        });
        if let Some(did) = device_id {
            body["device_id"] = did.into();
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            debug!(localpart, "puppet registered");
            Ok(user_id)
        } else if status.as_u16() == 400 {
            let resp_body: Value = resp.json().await.unwrap_or_default();
            let errcode = resp_body
                .get("errcode")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if errcode == "M_USER_IN_USE" {
                debug!(localpart, "puppet already registered");
                // If user exists but we need a device, ensure it via login.
                if let Some(did) = device_id {
                    self.ensure_device_via_login(localpart, did).await?;
                }
                Ok(user_id)
            } else {
                anyhow::bail!("register puppet failed: {resp_body}");
            }
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("register puppet failed ({status}): {text}");
        }
    }

    /// Ensure a device exists for an appservice user by performing a login.
    /// This is needed when the user already exists but the device hasn't been created yet.
    async fn ensure_device_via_login(
        &self,
        localpart: &str,
        device_id: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/_matrix/client/v3/login", self.homeserver_url);

        let body = json!({
            "type": "m.login.application_service",
            "identifier": {
                "type": "m.id.user",
                "user": localpart,
            },
            "device_id": device_id,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            debug!(localpart, device_id, "device ensured via login");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("ensure device via login failed: {text}");
        }
    }

    /// Set the display name for a puppet user.
    pub async fn set_display_name(&self, user_id: &str, name: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/profile/{}/displayname",
            self.homeserver_url,
            urlencoding::encode(user_id)
        );

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .query(&[("user_id", user_id)])
            .json(&json!({ "displayname": name }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!(user_id, "failed to set display name: {text}");
        }
        Ok(())
    }

    /// Set the avatar for a puppet user.
    pub async fn set_avatar(&self, user_id: &str, avatar_url: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/profile/{}/avatar_url",
            self.homeserver_url,
            urlencoding::encode(user_id)
        );

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .query(&[("user_id", user_id)])
            .json(&json!({ "avatar_url": avatar_url }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!(user_id, "failed to set avatar: {text}");
        }
        Ok(())
    }

    /// Join a room as a puppet user.
    pub async fn join_room(&self, room_id: &str, as_user: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/join/{}",
            self.homeserver_url,
            urlencoding::encode(room_id)
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&[("user_id", as_user)])
            .json(&json!({}))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("failed to join room {room_id} as {as_user}: {text}");
        }
        Ok(())
    }

    /// Create a new Matrix room as the bridge bot.
    ///
    /// Returns the room ID. The bridge bot is automatically a member (creator).
    /// When `encrypted` is true, the room is created with m.room.encryption.
    pub async fn create_room(
        &self,
        name: Option<&str>,
        invite: &[&str],
        encrypted: bool,
    ) -> anyhow::Result<String> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver_url);

        let mut initial_state: Vec<Value> = Vec::new();
        if encrypted {
            initial_state.push(json!({
                "type": "m.room.encryption",
                "state_key": "",
                "content": { "algorithm": "m.megolm.v1.aes-sha2" },
            }));
        }

        let mut body = json!({
            "preset": "private_chat",
            "initial_state": initial_state,
            "invite": invite,
        });
        if let Some(n) = name {
            body["name"] = n.into();
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            let data: Value = resp.json().await?;
            let room_id = data
                .get("room_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("create room response missing room_id: {data}"))?
                .to_string();
            debug!(room_id, "room created");
            Ok(room_id)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("create room failed: {text}");
        }
    }

    /// Invite a user to a room (as the bridge bot).
    ///
    /// Silently succeeds if the user is already in the room (`M_FORBIDDEN`
    /// with "already in the room" from Synapse).
    pub async fn invite_user(&self, room_id: &str, user_id: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/invite",
            self.homeserver_url,
            urlencoding::encode(room_id)
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .json(&json!({ "user_id": user_id }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            // Synapse returns M_FORBIDDEN with "already in the room" when the
            // user has already joined.  Check both errcode and message to avoid
            // swallowing unrelated M_FORBIDDEN errors (e.g. missing permission).
            if let Ok(body) = serde_json::from_str::<Value>(&text) {
                let errcode = body.get("errcode").and_then(|v| v.as_str()).unwrap_or("");
                if errcode == "M_FORBIDDEN" && text.contains("already in the room") {
                    debug!("{user_id} already in {room_id}, skipping invite");
                    return Ok(());
                }
            }
            anyhow::bail!("failed to invite {user_id} to {room_id}: {text}");
        }
        Ok(())
    }

    /// Send a message event to a room as a puppet user.
    pub async fn send_message(
        &self,
        room_id: &str,
        content: &Value,
        as_user: &str,
        txn_id: &str,
    ) -> anyhow::Result<String> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.homeserver_url,
            urlencoding::encode(room_id),
            urlencoding::encode(txn_id),
        );

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .query(&[("user_id", as_user)])
            .json(content)
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            let event_id = body
                .get("event_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(event_id)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("send message failed: {text}");
        }
    }

    /// Send an encrypted event to a room as a puppet user.
    pub async fn send_encrypted_message(
        &self,
        room_id: &str,
        content: &Value,
        as_user: &str,
        txn_id: &str,
    ) -> anyhow::Result<String> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.encrypted/{}",
            self.homeserver_url,
            urlencoding::encode(room_id),
            urlencoding::encode(txn_id),
        );

        let mut query: Vec<(&str, String)> = vec![("user_id", as_user.to_string())];
        // MSC3202/MSC4190: include device_id so Synapse knows which device
        // encrypted the content (bridge bot's single device).
        if let Some(ref did) = self.device_id {
            query.push(("device_id", did.clone()));
        }

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .query(&query)
            .json(content)
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            let event_id = body
                .get("event_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(event_id)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("send encrypted message failed: {text}");
        }
    }

    /// Enable encryption on a room by sending the m.room.encryption state event.
    pub async fn enable_room_encryption(&self, room_id: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.encryption/",
            self.homeserver_url,
            urlencoding::encode(room_id),
        );

        let content = json!({
            "algorithm": "m.megolm.v1.aes-sha2",
        });

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .json(&content)
            .send()
            .await?;

        if resp.status().is_success() || resp.status().as_u16() == 409 {
            // 409 = already set, which is fine
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("enable room encryption failed: {text}");
        }
    }

    /// Upload media to the homeserver. Returns the mxc:// URI.
    pub async fn upload_media(
        &self,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        let url = format!("{}/_matrix/media/v3/upload", self.homeserver_url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&[("filename", filename)])
            .header("content-type", content_type)
            .body(data)
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            let mxc = body
                .get("content_uri")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(mxc)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("media upload failed: {text}");
        }
    }

    // --- E2EE key management methods ---

    /// Build query params for E2EE requests (MSC3202 device masquerading).
    fn e2ee_query_params(&self) -> Vec<(&str, String)> {
        let mut params = Vec::new();
        if let Some(ref uid) = self.bot_user_id {
            params.push(("user_id", uid.clone()));
        }
        if let Some(ref did) = self.device_id {
            params.push(("device_id", did.clone()));
        }
        params
    }

    /// Upload device keys and one-time keys to the homeserver.
    pub async fn upload_keys_raw(
        &self,
        req: &KeysUploadRequest,
    ) -> anyhow::Result<KeysUploadResponse> {
        let url = format!("{}/_matrix/client/v3/keys/upload", self.homeserver_url);

        let body = json!({
            "device_keys": req.device_keys,
            "one_time_keys": req.one_time_keys,
            "fallback_keys": req.fallback_keys,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        let http_resp = Self::to_http_response(resp).await?;
        let response = KeysUploadResponse::try_from_http_response(http_resp)?;
        Ok(response)
    }

    /// Query device keys for users.
    pub async fn query_keys_raw(
        &self,
        req: &KeysQueryRequest,
    ) -> anyhow::Result<KeysQueryResponse> {
        let url = format!("{}/_matrix/client/v3/keys/query", self.homeserver_url);

        let body = json!({
            "device_keys": req.device_keys,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        let http_resp = Self::to_http_response(resp).await?;
        let response = KeysQueryResponse::try_from_http_response(http_resp)?;
        Ok(response)
    }

    /// Claim one-time keys for establishing Olm sessions.
    pub async fn claim_keys_raw(
        &self,
        req: &KeysClaimRequest,
    ) -> anyhow::Result<KeysClaimResponse> {
        let url = format!("{}/_matrix/client/v3/keys/claim", self.homeserver_url);

        let body = json!({
            "one_time_keys": req.one_time_keys,
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        let http_resp = Self::to_http_response(resp).await?;
        let response = KeysClaimResponse::try_from_http_response(http_resp)?;
        Ok(response)
    }

    /// Send a to-device event (key exchange, etc.).
    pub async fn send_to_device_raw(
        &self,
        req: &ToDeviceRequest,
    ) -> anyhow::Result<ToDeviceResponse> {
        let url = format!(
            "{}/_matrix/client/v3/sendToDevice/{}/{}",
            self.homeserver_url,
            urlencoding::encode(&req.event_type.to_string()),
            urlencoding::encode(req.txn_id.as_ref()),
        );

        let body = json!({ "messages": req.messages });

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        let http_resp = Self::to_http_response(resp).await?;
        let response = ToDeviceResponse::try_from_http_response(http_resp)?;
        Ok(response)
    }

    /// Get the power level of a user in a room.
    /// Returns the user's power level (default 0 if not found).
    pub async fn get_user_power_level(&self, room_id: &str, user_id: &str) -> anyhow::Result<i64> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.power_levels/",
            self.homeserver_url,
            urlencoding::encode(room_id),
        );

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.as_token)
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            let users_default = body
                .get("users_default")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let level = body
                .get("users")
                .and_then(|v| v.get(user_id))
                .and_then(|v| v.as_i64())
                .unwrap_or(users_default);
            Ok(level)
        } else {
            Ok(0)
        }
    }

    /// Query the m.room.encryption state event for a room.
    ///
    /// Returns `Ok(Some(content))` if the room is encrypted, `Ok(None)` if not,
    /// or an error if the request failed.
    pub async fn get_room_encryption_event(&self, room_id: &str) -> anyhow::Result<Option<Value>> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.encryption/",
            self.homeserver_url,
            urlencoding::encode(room_id),
        );

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.as_token)
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            Ok(Some(body))
        } else if resp.status().as_u16() == 404 {
            Ok(None)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("get room encryption state failed: {text}");
        }
    }

    /// Upload cross-signing keys (master, self-signing, user-signing).
    ///
    /// Corresponds to `POST /_matrix/client/v3/keys/device_signing/upload`.
    /// Appservice requests typically bypass UIA, so `auth` is not sent.
    pub async fn upload_signing_keys(
        &self,
        master_key: Option<&Value>,
        self_signing_key: Option<&Value>,
        user_signing_key: Option<&Value>,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/keys/device_signing/upload",
            self.homeserver_url
        );

        let mut body = json!({});
        if let Some(k) = master_key {
            body["master_key"] = k.clone();
        }
        if let Some(k) = self_signing_key {
            body["self_signing_key"] = k.clone();
        }
        if let Some(k) = user_signing_key {
            body["user_signing_key"] = k.clone();
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            debug!("cross-signing keys uploaded");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("upload signing keys failed: {text}");
        }
    }

    /// Upload cross-signing signatures (device key signatures, etc.).
    ///
    /// Corresponds to `POST /_matrix/client/v3/keys/signatures/upload`.
    pub async fn upload_signatures(
        &self,
        signed_keys: &Value,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/keys/signatures/upload",
            self.homeserver_url
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.as_token)
            .query(&self.e2ee_query_params())
            .json(signed_keys)
            .send()
            .await?;

        if resp.status().is_success() {
            debug!("signatures uploaded");
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("upload signatures failed: {text}");
        }
    }

    /// Get the members of a room.
    pub async fn get_room_members(&self, room_id: &str) -> anyhow::Result<Vec<String>> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/members",
            self.homeserver_url,
            urlencoding::encode(room_id),
        );

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.as_token)
            .query(&[("membership", "join")])
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            let members = body
                .get("chunk")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            e.get("state_key")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(members)
        } else {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("get room members failed: {text}");
        }
    }
}
