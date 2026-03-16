use secrecy::ExposeSecret;
use serde_json::{Value, json};
use tracing::debug;

use super::MatrixClient;

impl MatrixClient {
    /// Create a new Matrix Space as the bridge bot.
    ///
    /// A Space is a room with `m.space` creation_content type.
    /// Returns the space room ID.
    pub async fn create_space(&self, name: &str, topic: Option<&str>) -> anyhow::Result<String> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver_url);

        let mut body = json!({
            "name": name,
            "preset": "private_chat",
            "creation_content": {
                "type": "m.space"
            },
            "initial_state": [
                {
                    "type": "m.room.history_visibility",
                    "state_key": "",
                    "content": { "history_visibility": "invited" }
                }
            ],
            "power_level_content_override": {
                "invite": 50
            }
        });

        if let Some(t) = topic {
            body["topic"] = t.into();
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.as_token.expose_secret())
            .json(&body)
            .send()
            .await?;

        let resp = Self::check_response(resp, "create space").await?;
        let data: Value = resp.json().await?;
        let space_id = data
            .get("room_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("create space response missing room_id: {data}"))?
            .to_string();
        debug!(space_id, name, "space created");
        Ok(space_id)
    }

    /// Add a room as a child of a Space via the `m.space.child` state event.
    ///
    /// This makes the room appear inside the Space in Matrix clients.
    pub async fn set_space_child(&self, space_id: &str, child_room_id: &str) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.space.child/{}",
            self.homeserver_url,
            urlencoding::encode(space_id),
            urlencoding::encode(child_room_id),
        );

        let content = json!({
            "via": [&self.server_name],
        });

        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.as_token.expose_secret())
            .json(&content)
            .send()
            .await?;

        Self::check_response(
            resp,
            &format!("set space child {child_room_id} in {space_id}"),
        )
        .await?;

        debug!(space_id, child_room_id, "room added to space");
        Ok(())
    }

    /// Remove a room from a Space by clearing the `m.space.child` state event.
    pub async fn remove_space_child(
        &self,
        space_id: &str,
        child_room_id: &str,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.space.child/{}",
            self.homeserver_url,
            urlencoding::encode(space_id),
            urlencoding::encode(child_room_id),
        );

        let content = json!({});

        let resp = self
            .client
            .put(&url)
            .bearer_auth(self.as_token.expose_secret())
            .json(&content)
            .send()
            .await?;

        Self::check_response(
            resp,
            &format!("remove space child {child_room_id} from {space_id}"),
        )
        .await?;

        debug!(space_id, child_room_id, "room removed from space");
        Ok(())
    }
}
