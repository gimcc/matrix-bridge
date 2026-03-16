use serde_json::Value;
use tracing::{debug, error, info, warn};

use super::Dispatcher;

impl Dispatcher {
    /// Handle a batch of events from the homeserver transaction endpoint.
    pub async fn handle_transaction(&self, events: &[Value]) {
        for event in events {
            if let Err(e) = self.handle_event(event).await {
                error!(error = %e, "failed to handle event");
            }
        }
    }

    /// Handle a single Matrix event.
    async fn handle_event(&self, event: &Value) -> anyhow::Result<()> {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let room_id = event.get("room_id").and_then(|v| v.as_str()).unwrap_or("");
        let sender = event.get("sender").and_then(|v| v.as_str()).unwrap_or("");

        // m.room.member events must be processed even when sent by the bot
        // itself (e.g. self-invite), so check membership before the bot skip.
        if event_type == "m.room.member" {
            return self.handle_membership(room_id, event).await;
        }

        // Skip events from the bridge bot itself (not puppet users — those need
        // cross-platform forwarding).
        if self.is_bridge_bot(sender) {
            return Ok(());
        }

        match event_type {
            "m.room.message" => self.handle_room_message(room_id, sender, event).await,
            "m.room.encrypted" => self.handle_encrypted_event(room_id, sender, event).await,
            "m.room.encryption" => {
                self.track_room_encryption(room_id).await;
                Ok(())
            }
            "m.room.redaction" => self.handle_redaction(room_id, sender, event).await,
            _ => {
                debug!(event_type, room_id, "ignoring event type");
                Ok(())
            }
        }
    }

    /// Handle m.room.member events — auto-accept invites for the bridge bot
    /// and puppet users.
    async fn handle_membership(
        &self,
        room_id: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let membership = event
            .get("content")
            .and_then(|c| c.get("membership"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let state_key = event
            .get("state_key")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if state_key.is_empty() {
            return Ok(());
        }

        // Track device keys for new joins/invites in encrypted rooms.
        if membership == "join" || membership == "invite" {
            self.track_member_device(room_id, state_key).await;
        }

        // Only process invites for auto-accept logic.
        if membership != "invite" {
            return Ok(());
        }

        let is_bot = state_key == self.bot_user_id;
        let is_puppet = state_key.starts_with(&self.puppet_user_prefix);

        if !is_bot && !is_puppet {
            return Ok(());
        }

        let inviter = event.get("sender").and_then(|v| v.as_str()).unwrap_or("");
        let is_bridge_bot_inviting = inviter == self.bot_user_id;
        if !is_bridge_bot_inviting && !self.permissions.is_invite_allowed(inviter) {
            let target = if is_bot { "bot" } else { "puppet" };
            warn!(
                room_id,
                inviter, target, "invite rejected: sender not in invite_whitelist"
            );
            return Ok(());
        }

        info!(
            room_id,
            invited_user = state_key,
            is_bot,
            "auto-accepting room invite"
        );

        if let Err(e) = self.matrix_client.join_room(room_id, state_key).await {
            warn!(
                room_id,
                invited_user = state_key,
                error = %e,
                "failed to auto-accept invite"
            );
            return Ok(());
        }

        // When the bot joins a room, track all room members' devices.
        if is_bot {
            self.track_room_members_if_encrypted(room_id).await;
        }

        Ok(())
    }

    /// Handle an m.room.encrypted event — decrypt and process the inner event.
    ///
    /// If the bot fails to decrypt, attempts fallback decryption by processing
    /// outgoing key requests (similar to matrix-bot-sdk's retry logic).
    async fn handle_encrypted_event(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let Some(pool) = &self.crypto_pool else {
            debug!(room_id, "received encrypted event but E2EE is not enabled");
            return Ok(());
        };

        // Ensure room is tracked and members' devices are up to date.
        self.ensure_room_encrypted(room_id).await;
        self.track_all_room_members(room_id).await;

        let event_id = event
            .get("event_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let ruma_room_id: &ruma::RoomId = room_id.try_into()?;

        // Attempt decryption (bot's OlmMachine, which should have Megolm session keys).
        let decrypted = match pool.decrypt(ruma_room_id, event).await {
            Ok(d) => d,
            Err(e) => {
                // Process any outgoing key requests generated by the failed decrypt.
                if let Err(e2) = pool.bot().process_outgoing_requests().await {
                    warn!(
                        room_id,
                        "failed to process key requests after decrypt failure: {e2}"
                    );
                }

                error!(
                    room_id,
                    sender, event_id, error = %e, "failed to decrypt event (message will be dropped)"
                );
                return Ok(());
            }
        };

        match decrypted.event_type.as_str() {
            "m.room.message" => {
                let mut pseudo_event = event.clone();
                pseudo_event["type"] = "m.room.message".into();
                pseudo_event["content"] = decrypted.content;
                if !decrypted.sender.is_empty() {
                    pseudo_event["sender"] = decrypted.sender.into();
                }
                pseudo_event["trust_level"] = decrypted.trust_level.into();
                self.handle_room_message(room_id, sender, &pseudo_event)
                    .await
            }
            other => {
                debug!(event_type = other, room_id, "ignoring decrypted event type");
                Ok(())
            }
        }
    }
}
