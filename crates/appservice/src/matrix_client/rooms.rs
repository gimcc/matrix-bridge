use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tracing::debug;

use super::MatrixClient;

impl MatrixClient {
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("user_id", as_user)])
            .json(&json!({}))
            .send()
            .await?;

        Self::check_response(resp, &format!("join room {room_id} as {as_user}")).await?;
        Ok(())
    }

    /// Leave a room as the bridge bot (best-effort cleanup).
    pub async fn leave_room_as_bot(&self, room_id: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/leave",
            self.homeserver_url,
            urlencoding::encode(room_id)
        );

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.as_token.expose_secret())
            .json(&serde_json::json!({}))
            .send()
            .await?;

        Self::check_response(resp, &format!("leave room {room_id}")).await?;
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
            .bearer_auth(self.as_token.expose_secret())
            .json(&body)
            .send()
            .await?;

        let resp = Self::check_response(resp, "create room").await?;
        let data: Value = resp.json().await?;
        let room_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("create room response missing room_id: {data}"))?
            .to_string();
        debug!(room_id, "room created");
        Ok(room_id)
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
            .bearer_auth(self.as_token.expose_secret())
            .json(&json!({ "user_id": user_id }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await?;
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
            .bearer_auth(self.as_token.expose_secret())
            .json(&content)
            .send()
            .await?;

        if resp.status().is_success() || resp.status().as_u16() == 409 {
            // 409 = already set, which is fine
            Ok(())
        } else {
            let text = resp.text().await?;
            anyhow::bail!("enable room encryption failed: {text}");
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
            .bearer_auth(self.as_token.expose_secret())
            .send()
            .await?;

        if resp.status().is_success() {
            let body: Value = resp.json().await?;
            Ok(Some(body))
        } else if resp.status().as_u16() == 404 {
            Ok(None)
        } else {
            let text = resp.text().await?;
            anyhow::bail!("get room encryption state failed: {text}");
        }
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
            .bearer_auth(self.as_token.expose_secret())
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("membership", "join")])
            .send()
            .await?;

        let resp = Self::check_response(resp, "get room members").await?;
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
    }
}
