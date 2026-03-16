use matrix_sdk_crypto::{DecryptionSettings, TrustRequirement};
use ruma::RoomId;
use ruma::serde::Raw;
use serde_json::Value;

use super::CryptoManager;

/// Result of decrypting an event.
pub struct DecryptedEvent {
    pub event_type: String,
    pub content: Value,
    pub sender: String,
    /// Verification trust level of the sender's device.
    pub trust_level: String,
}

impl CryptoManager {
    /// Decrypt an m.room.encrypted event.
    ///
    /// Returns the decrypted event content and type.
    pub async fn decrypt(&self, room_id: &RoomId, event: &Value) -> anyhow::Result<DecryptedEvent> {
        let raw = serde_json::value::to_raw_value(event)?;
        let raw_event = Raw::from_json(raw);

        let decryption_settings = DecryptionSettings {
            sender_device_trust_requirement: TrustRequirement::Untrusted,
        };

        let decrypted = self
            .machine
            .decrypt_room_event(&raw_event, room_id, &decryption_settings)
            .await?;

        let event_json: Value = serde_json::from_str(decrypted.event.json().get())?;

        let trust_level = {
            use matrix_sdk_common::deserialized_responses::VerificationState;
            match decrypted.encryption_info.verification_state {
                VerificationState::Verified => "verified",
                _ => "unverified",
            }
        }
        .to_string();

        Ok(DecryptedEvent {
            event_type: event_json
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            content: event_json.get("content").cloned().unwrap_or_default(),
            sender: event_json
                .get("sender")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            trust_level,
        })
    }
}
