use std::collections::BTreeMap;
use std::path::Path;

use matrix_sdk_crypto::{
    DecryptionSettings, EncryptionSettings, EncryptionSyncChanges, OlmMachine, TrustRequirement,
    store::types::RoomSettings, types::EventEncryptionAlgorithm,
    types::requests::AnyOutgoingRequest,
};
use matrix_sdk_sqlite::SqliteCryptoStore;
use ruma::{
    OneTimeKeyAlgorithm, OwnedUserId, RoomId, UInt, UserId,
    api::client::sync::sync_events::DeviceLists, events::AnyToDeviceEvent, serde::Raw,
};
use serde_json::Value;
use tracing::{debug, info};

use matrix_bridge_core::config::EncryptionConfig;

use crate::matrix_client::MatrixClient;

/// Manages the bridge bot's single cryptographic device.
///
/// Uses OlmMachine from matrix-sdk-crypto to handle:
/// - Olm session management (1:1 key exchange)
/// - Megolm session management (group encryption/decryption)
/// - Device key uploads
/// - To-device event processing (key exchange via MSC2409)
/// - Device list tracking (MSC3202)
pub struct CryptoManager {
    machine: OlmMachine,
    matrix_client: MatrixClient,
}

impl CryptoManager {
    /// Initialize the crypto manager for the bridge bot.
    ///
    /// Creates or loads the OlmMachine from a persistent SQLite crypto store.
    pub async fn new(
        user_id: &UserId,
        device_id: &ruma::DeviceId,
        config: &EncryptionConfig,
        matrix_client: MatrixClient,
    ) -> anyhow::Result<Self> {
        let store_path = Path::new(&config.crypto_store);
        std::fs::create_dir_all(store_path)?;

        let passphrase = config.crypto_store_passphrase.as_deref();
        if passphrase.is_none() || passphrase == Some("") {
            anyhow::bail!(
                "encryption.crypto_store_passphrase must be set when encryption is enabled. \
                 Without a passphrase, Olm/Megolm keys are stored unencrypted on disk."
            );
        }

        let store = SqliteCryptoStore::open(store_path, passphrase).await?;

        let machine = OlmMachine::with_store(user_id, device_id, store, None).await?;

        info!(
            user_id = %user_id,
            device_id = %device_id,
            "crypto manager initialized"
        );

        Ok(Self {
            machine,
            matrix_client,
        })
    }

    /// Upload device keys and one-time keys to the homeserver.
    ///
    /// Must be called on first startup and periodically when OTKs run low.
    /// Also processes any pending outgoing requests (key queries, claims, to-device).
    pub async fn process_outgoing_requests(&self) -> anyhow::Result<()> {
        let outgoing = self.machine.outgoing_requests().await?;

        for request in outgoing {
            match request.request() {
                AnyOutgoingRequest::KeysUpload(req) => {
                    let resp = self.matrix_client.upload_keys_raw(req).await?;
                    self.machine
                        .mark_request_as_sent(request.request_id(), &resp)
                        .await?;
                    debug!("device keys uploaded");
                }
                AnyOutgoingRequest::KeysQuery(req) => {
                    let resp = self.matrix_client.query_keys_raw(req).await?;
                    self.machine
                        .mark_request_as_sent(request.request_id(), &resp)
                        .await?;
                    debug!("keys query completed");
                }
                AnyOutgoingRequest::KeysClaim(req) => {
                    let resp = self.matrix_client.claim_keys_raw(req).await?;
                    self.machine
                        .mark_request_as_sent(request.request_id(), &resp)
                        .await?;
                    debug!("keys claimed");
                }
                AnyOutgoingRequest::ToDeviceRequest(req) => {
                    let resp = self.matrix_client.send_to_device_raw(req).await?;
                    self.machine
                        .mark_request_as_sent(request.request_id(), &resp)
                        .await?;
                    debug!("to-device event sent");
                }
                _ => {
                    debug!("unhandled outgoing request type");
                }
            }
        }

        Ok(())
    }

    /// Process to-device events received via appservice transactions (MSC2409).
    ///
    /// These events contain Olm key exchange messages needed for E2EE.
    pub async fn receive_sync_changes(
        &self,
        to_device_events: Vec<Raw<AnyToDeviceEvent>>,
        changed_devices: &DeviceLists,
        one_time_keys_counts: &BTreeMap<OneTimeKeyAlgorithm, UInt>,
        unused_fallback_keys: Option<&[OneTimeKeyAlgorithm]>,
    ) -> anyhow::Result<()> {
        let decryption_settings = DecryptionSettings {
            sender_device_trust_requirement: TrustRequirement::Untrusted,
        };

        self.machine
            .receive_sync_changes(
                EncryptionSyncChanges {
                    to_device_events,
                    changed_devices,
                    one_time_keys_counts,
                    unused_fallback_keys,
                    next_batch_token: None,
                },
                &decryption_settings,
            )
            .await?;

        // Process any outgoing requests generated by the sync changes
        // (e.g., key claims, to-device responses).
        self.process_outgoing_requests().await?;

        Ok(())
    }

    /// Encrypt a message for a room.
    ///
    /// Returns the encrypted event content as JSON (m.room.encrypted).
    /// If the room doesn't have an encryption session yet, creates one.
    pub async fn encrypt(
        &self,
        room_id: &RoomId,
        event_type: &str,
        content: &Value,
        room_members: &[OwnedUserId],
    ) -> anyhow::Result<Value> {
        // Ensure we have a Megolm session for this room.
        // Share room key with all devices of room members.
        let requests = self
            .machine
            .share_room_key(
                room_id,
                room_members.iter().map(|u| u.as_ref()),
                EncryptionSettings::default(),
            )
            .await?;

        for request in requests {
            let resp = self.matrix_client.send_to_device_raw(&request).await?;
            self.machine
                .mark_request_as_sent(&request.txn_id, &resp)
                .await?;
        }

        // Now encrypt the content.
        let raw_content = serde_json::value::to_raw_value(content)?;
        let raw_typed: Raw<ruma::events::AnyMessageLikeEventContent> = Raw::from_json(raw_content);

        let encrypted = self
            .machine
            .encrypt_room_event_raw(room_id, event_type, &raw_typed)
            .await?;

        let encrypted_value: Value = serde_json::from_str(encrypted.json().get())?;
        Ok(encrypted_value)
    }

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
        })
    }

    /// Check if a room has encryption enabled.
    pub async fn is_room_encrypted(&self, room_id: &RoomId) -> bool {
        self.machine
            .room_settings(room_id)
            .await
            .ok()
            .flatten()
            .is_some()
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

    /// Get the bridge bot's device ID.
    pub fn device_id(&self) -> &ruma::DeviceId {
        self.machine.device_id()
    }

    /// Get the bridge bot's user ID.
    pub fn user_id(&self) -> &UserId {
        self.machine.user_id()
    }
}

/// Result of decrypting an event.
pub struct DecryptedEvent {
    pub event_type: String,
    pub content: Value,
    pub sender: String,
}
