use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tracing::{debug, warn};

use super::MatrixClient;

impl MatrixClient {
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
            .bearer_auth(self.as_token.expose_secret())
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            debug!(localpart, "puppet registered");
            Ok(user_id)
        } else if status.as_u16() == 400 {
            let resp_body: Value = resp.json().await?;
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
            let text = resp.text().await?;
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
            .bearer_auth(self.as_token.expose_secret())
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            debug!(localpart, device_id, "device ensured via login");
            Ok(())
        } else {
            let text = resp.text().await?;
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("user_id", user_id)])
            .json(&json!({ "displayname": name }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await?;
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("user_id", user_id)])
            .json(&json!({ "avatar_url": avatar_url }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await?;
            warn!(user_id, "failed to set avatar: {text}");
        }
        Ok(())
    }
}
