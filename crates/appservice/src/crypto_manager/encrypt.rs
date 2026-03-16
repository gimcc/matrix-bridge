use matrix_sdk_crypto::{EncryptionSettings, store::types::RoomSettings};
use matrix_sdk_crypto::types::EventEncryptionAlgorithm;
use ruma::{OwnedUserId, RoomId, UserId};
use ruma::serde::Raw;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::matrix_client::MatrixClient;

use super::CryptoManager;

impl CryptoManager {
    /// Encrypt a message for a room.
    ///
    /// Performs the full preparation flow before encrypting:
    /// 1. Update tracked users for all room members
    /// 2. Process ALL pending outgoing requests (keys query, claim, etc.)
    /// 3. Claim missing Olm sessions
    /// 4. Share Megolm room key with all member devices
    /// 5. Encrypt the event content
    ///
    /// Returns the encrypted event content as JSON (m.room.encrypted).
    pub async fn encrypt(
        &self,
        room_id: &RoomId,
        event_type: &str,
        content: &Value,
        room_members: &[OwnedUserId],
    ) -> anyhow::Result<Value> {
        let _guard = self.lock.write().await;

        debug!(
            room_id = %room_id,
            member_count = room_members.len(),
            "encrypt: starting preparation"
        );

        // Step 1: Ensure all room members are tracked.
        let refs: Vec<&UserId> = room_members.iter().map(|u| u.as_ref()).collect();
        self.machine.update_tracked_users(refs).await?;

        // Step 2: Process pending outgoing requests.
        self.process_outgoing_requests().await?;

        // Step 3: Claim missing Olm sessions for room members.
        self.claim_missing_sessions(room_members).await?;

        // Step 4: Share Megolm room key with all devices of room members.
        let requests = self
            .machine
            .share_room_key(
                room_id,
                room_members.iter().map(|u| u.as_ref()),
                EncryptionSettings::default(),
            )
            .await?;

        debug!(
            room_id = %room_id,
            to_device_requests = requests.len(),
            "encrypt: sharing room key"
        );

        for request in &requests {
            let resp = self.matrix_client.send_to_device_raw(request).await?;
            self.machine
                .mark_request_as_sent(&request.txn_id, &resp)
                .await?;
        }

        // Step 5: Encrypt the content.
        let raw_content = serde_json::value::to_raw_value(content)?;
        let raw_typed: Raw<ruma::events::AnyMessageLikeEventContent> = Raw::from_json(raw_content);

        let encrypted = self
            .machine
            .encrypt_room_event_raw(room_id, event_type, &raw_typed)
            .await?;

        let encrypted_value: Value = serde_json::from_str(encrypted.json().get())?;

        debug!(
            room_id = %room_id,
            session_id = encrypted_value.get("session_id").and_then(|v| v.as_str()).unwrap_or("?"),
            "encrypt: done"
        );

        Ok(encrypted_value)
    }

    /// Track a room's encryption state.
    ///
    /// Call this when we see an m.room.encryption state event.
    pub async fn set_room_encrypted(&self, room_id: &RoomId) -> anyhow::Result<()> {
        let settings = RoomSettings {
            algorithm: EventEncryptionAlgorithm::MegolmV1AesSha2,
            only_allow_trusted_devices: false,
            session_rotation_period: None,
            session_rotation_period_messages: None,
        };

        self.machine.set_room_settings(room_id, &settings).await?;

        info!(room_id = %room_id, "room marked as encrypted");
        Ok(())
    }

    /// Check if a room has encryption enabled in the local crypto store.
    pub async fn is_room_encrypted_local(&self, room_id: &RoomId) -> bool {
        self.machine
            .room_settings(room_id)
            .await
            .ok()
            .flatten()
            .is_some()
    }

    /// Check if a room has encryption enabled by querying the homeserver.
    ///
    /// Falls back to local crypto store if the server query fails.
    /// If the room is encrypted on the server but not locally tracked,
    /// automatically marks it as encrypted.
    pub async fn is_room_encrypted(&self, room_id: &RoomId, matrix_client: &MatrixClient) -> bool {
        // First check local store (fast path).
        if self.is_room_encrypted_local(room_id).await {
            return true;
        }

        // Query the server for the room encryption state event.
        match matrix_client
            .get_room_encryption_event(room_id.as_str())
            .await
        {
            Ok(Some(_)) => {
                if let Err(e) = self.set_room_encrypted(room_id).await {
                    warn!(room_id = %room_id, "failed to sync room encryption state: {e}");
                }
                true
            }
            Ok(None) => false,
            Err(e) => {
                debug!(room_id = %room_id, "failed to query room encryption state: {e}");
                false
            }
        }
    }
}
