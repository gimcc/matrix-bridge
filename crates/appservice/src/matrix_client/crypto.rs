use secrecy::ExposeSecret;
use ruma::api::IncomingResponse;
use ruma::api::client::{
    keys::{
        claim_keys::v3::Response as KeysClaimResponse,
        get_keys::v3::Response as KeysQueryResponse,
        upload_keys::v3::Response as KeysUploadResponse,
    },
    to_device::send_event_to_device::v3::Response as ToDeviceResponse,
};
use serde_json::{Value, json};
use tracing::debug;

use matrix_sdk_crypto::types::requests::{KeysQueryRequest, ToDeviceRequest};
use ruma::api::client::keys::{
    claim_keys::v3::Request as KeysClaimRequest, upload_keys::v3::Request as KeysUploadRequest,
};

use super::MatrixClient;

impl MatrixClient {
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
            .bearer_auth(self.as_token.expose_secret())
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
            .bearer_auth(self.as_token.expose_secret())
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
            .bearer_auth(self.as_token.expose_secret())
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        let http_resp = Self::to_http_response(resp).await?;
        let response = ToDeviceResponse::try_from_http_response(http_resp)?;
        Ok(response)
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&self.e2ee_query_params())
            .json(&body)
            .send()
            .await?;

        Self::check_response(resp, "upload signing keys").await?;
        debug!("cross-signing keys uploaded");
        Ok(())
    }

    /// Upload cross-signing signatures (device key signatures, etc.).
    ///
    /// Corresponds to `POST /_matrix/client/v3/keys/signatures/upload`.
    pub async fn upload_signatures(&self, signed_keys: &Value) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/keys/signatures/upload",
            self.homeserver_url
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.as_token.expose_secret())
            .query(&self.e2ee_query_params())
            .json(signed_keys)
            .send()
            .await?;

        Self::check_response(resp, "upload signatures").await?;
        debug!("signatures uploaded");
        Ok(())
    }
}
