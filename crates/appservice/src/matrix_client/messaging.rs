use secrecy::ExposeSecret;
use serde_json::Value;

use super::MatrixClient;

impl MatrixClient {
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&[("user_id", as_user)])
            .json(content)
            .send()
            .await?;

        let resp = Self::check_response(resp, "send message").await?;
        Self::extract_event_id(resp).await
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
            .bearer_auth(self.as_token.expose_secret())
            .query(&query)
            .json(content)
            .send()
            .await?;

        let resp = Self::check_response(resp, "send encrypted message").await?;
        Self::extract_event_id(resp).await
    }
}
