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
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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
///
/// All crypto operations are serialized through an internal RwLock to prevent
/// concurrent sync/encrypt conflicts (similar to matrix-bot-sdk's AsyncLock).
pub struct CryptoManager {
    machine: OlmMachine,
    matrix_client: MatrixClient,
    /// Serializes sync-level operations (receive_sync_changes, process_outgoing)
    /// against encrypt-level operations (prepare + encrypt) to prevent races.
    lock: RwLock<()>,
}

impl CryptoManager {
    /// Initialize the crypto manager for the bridge bot.
    ///
    /// Creates or loads the OlmMachine from a persistent SQLite crypto store.
    /// After initialization, verifies that device keys are present on the
    /// homeserver. If missing (stale crypto store from a previous failed
    /// startup), the store is rebuilt from scratch.
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

        let mut cm = Self::open(user_id, device_id, store_path, passphrase, matrix_client.clone()).await?;

        // Upload any pending keys (fresh store) or process OTK replenishment.
        cm.process_outgoing_requests().await?;

        // Verify that our device keys are actually on the homeserver.
        // A restored crypto store may believe keys were uploaded when they
        // never actually reached the server (e.g. previous startup crashed
        // before the upload completed).  In that case, wipe the store and
        // re-create from scratch so a fresh upload is generated.
        if !cm.device_keys_on_server().await? {
            warn!("device keys missing on server — rebuilding crypto store");
            // Drop the machine so the SQLite handle is released.
            drop(cm);
            if store_path.exists() {
                std::fs::remove_dir_all(store_path)?;
                std::fs::create_dir_all(store_path)?;
            }
            cm = Self::open(user_id, device_id, store_path, passphrase, matrix_client).await?;
            cm.process_outgoing_requests().await?;

            if !cm.device_keys_on_server().await? {
                anyhow::bail!(
                    "failed to upload device keys after crypto store rebuild — \
                     check homeserver connectivity and appservice registration"
                );
            }
            info!("device keys uploaded after crypto store rebuild");
        } else {
            debug!("device keys verified on server");
        }

        Ok(cm)
    }

    /// Open or create the crypto store and build an OlmMachine.
    async fn open(
        user_id: &UserId,
        device_id: &ruma::DeviceId,
        store_path: &Path,
        passphrase: Option<&str>,
        matrix_client: MatrixClient,
    ) -> anyhow::Result<Self> {
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
            lock: RwLock::new(()),
        })
    }

    /// Check whether our device keys are present on the homeserver.
    async fn device_keys_on_server(&self) -> anyhow::Result<bool> {
        let user_id = self.machine.user_id().to_owned();
        let req = matrix_sdk_crypto::types::requests::KeysQueryRequest {
            device_keys: std::collections::BTreeMap::from([(user_id.clone(), Vec::new())]),
            timeout: Some(std::time::Duration::from_secs(10)),
        };
        let resp = self.matrix_client.query_keys_raw(&req).await?;

        let has_device = resp
            .device_keys
            .get(&user_id)
            .map_or(false, |devices| !devices.is_empty());

        Ok(has_device)
    }

    /// Upload device keys and one-time keys to the homeserver.
    ///
    /// Must be called on first startup and periodically when OTKs run low.
    /// Also processes any pending outgoing requests (key queries, claims, to-device).
    pub async fn process_outgoing_requests(&self) -> anyhow::Result<()> {
        let outgoing = self.machine.outgoing_requests().await?;

        for request in outgoing {
            self.dispatch_outgoing_request(request.request_id(), request.request()).await?;
        }

        Ok(())
    }

    /// Dispatch a single outgoing request to the homeserver and mark it as sent.
    async fn dispatch_outgoing_request(
        &self,
        request_id: &ruma::TransactionId,
        request: &AnyOutgoingRequest,
    ) -> anyhow::Result<()> {
        match request {
            AnyOutgoingRequest::KeysUpload(req) => {
                let otk_count = req.one_time_keys.len();
                let has_device_keys = req.device_keys.is_some();
                let resp = self.matrix_client.upload_keys_raw(req).await?;
                info!(
                    has_device_keys,
                    otk_count,
                    otk_counts = ?resp.one_time_key_counts,
                    "keys uploaded"
                );
                self.machine
                    .mark_request_as_sent(request_id, &resp)
                    .await?;
            }
            AnyOutgoingRequest::KeysQuery(req) => {
                let queried_users: Vec<String> = req.device_keys.keys()
                    .map(|u| u.to_string())
                    .collect();
                info!(users = ?queried_users, "keys query: requesting device keys");
                let resp = self.matrix_client.query_keys_raw(req).await?;
                // Log how many devices we got for each user.
                for (user_id, devices) in &resp.device_keys {
                    info!(
                        user_id = %user_id,
                        device_count = devices.len(),
                        device_ids = ?devices.keys().map(|d| d.to_string()).collect::<Vec<_>>(),
                        "keys query: got devices"
                    );
                }
                self.machine
                    .mark_request_as_sent(request_id, &resp)
                    .await?;
            }
            AnyOutgoingRequest::KeysClaim(req) => {
                let claim_users: Vec<String> = req.one_time_keys.keys()
                    .map(|u| u.to_string())
                    .collect();
                info!(users = ?claim_users, "keys claim: claiming OTKs");
                let resp = self.matrix_client.claim_keys_raw(req).await?;
                for (user_id, devices) in &resp.one_time_keys {
                    info!(
                        user_id = %user_id,
                        device_count = devices.len(),
                        "keys claim: got OTKs"
                    );
                }
                self.machine
                    .mark_request_as_sent(request_id, &resp)
                    .await?;
            }
            AnyOutgoingRequest::ToDeviceRequest(req) => {
                let recipient_count: usize = req.messages.values()
                    .map(|devices| devices.len())
                    .sum();
                debug!(
                    event_type = %req.event_type,
                    recipients = recipient_count,
                    "sending to-device event"
                );
                let resp = self.matrix_client.send_to_device_raw(req).await?;
                self.machine
                    .mark_request_as_sent(request_id, &resp)
                    .await?;
            }
            _ => {
                debug!("unhandled outgoing request type");
            }
        }
        Ok(())
    }

    /// Claim missing Olm sessions for the given users.
    ///
    /// Calls `OlmMachine::get_missing_sessions` and if a claim request is
    /// produced, sends it to the homeserver and marks it as sent.
    async fn claim_missing_sessions(&self, user_ids: &[OwnedUserId]) -> anyhow::Result<()> {
        let refs: Vec<&UserId> = user_ids.iter().map(|u| u.as_ref()).collect();
        let claim_req = self.machine.get_missing_sessions(refs.into_iter()).await?;

        if let Some((txn_id, req)) = claim_req {
            let claim_users: Vec<String> = req.one_time_keys.keys()
                .map(|u| u.to_string())
                .collect();
            info!(users = ?claim_users, "claiming missing Olm sessions");
            let resp = self.matrix_client.claim_keys_raw(&req).await?;
            self.machine.mark_request_as_sent(&txn_id, &resp).await?;
            info!("claimed missing Olm sessions successfully");
        } else {
            debug!("no missing Olm sessions to claim");
        }

        Ok(())
    }

    /// Process to-device events received via appservice transactions (MSC2409).
    ///
    /// These events contain Olm key exchange messages needed for E2EE.
    /// Acquires a write lock to prevent concurrent encrypt operations.
    pub async fn receive_sync_changes(
        &self,
        to_device_events: Vec<Raw<AnyToDeviceEvent>>,
        changed_devices: &DeviceLists,
        one_time_keys_counts: &BTreeMap<OneTimeKeyAlgorithm, UInt>,
        unused_fallback_keys: Option<&[OneTimeKeyAlgorithm]>,
    ) -> anyhow::Result<()> {
        let _guard = self.lock.write().await;

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
    /// Performs the full preparation flow before encrypting (like matrix-bot-sdk's
    /// `prepareEncrypt`):
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
        // Hold the write lock through the entire prepare + encrypt flow to
        // prevent concurrent receive_sync_changes from mutating Olm session
        // state between preparation and encryption.
        let _guard = self.lock.write().await;

        info!(
            room_id = %room_id,
            member_count = room_members.len(),
            members = ?room_members.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
            "encrypt: starting preparation"
        );

        // Step 1: Ensure all room members are tracked.
        let refs: Vec<&UserId> = room_members.iter().map(|u| u.as_ref()).collect();
        self.machine.update_tracked_users(refs).await?;

        // Step 2: Force a fresh device key query for all room members.
        // `update_tracked_users` is a no-op for already-tracked users, so we
        // use `query_keys_for_users` which always generates a new KeysQuery
        // regardless of tracking state.  This ensures we have up-to-date
        // device information even after bridge restarts or missed device-list
        // change notifications.
        {
            let (txn_id, keys_query_req) = self.machine.query_keys_for_users(
                room_members.iter().map(|u| u.as_ref()),
            );
            let queried_users: Vec<String> = keys_query_req.device_keys.keys()
                .map(|u| u.to_string())
                .collect();
            info!(users = ?queried_users, "encrypt: forced keys query for room members");
            let resp = self.matrix_client.query_keys_raw(&keys_query_req).await?;
            for (user_id, devices) in &resp.device_keys {
                info!(
                    user_id = %user_id,
                    device_count = devices.len(),
                    device_ids = ?devices.keys().map(|d| d.to_string()).collect::<Vec<_>>(),
                    "encrypt: got devices"
                );
            }
            self.machine.mark_request_as_sent(&txn_id, &resp).await?;
        }

        // Process any other pending outgoing requests (key uploads, etc.).
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

        info!(
            room_id = %room_id,
            to_device_requests = requests.len(),
            "encrypt: sharing room key"
        );

        for request in &requests {
            // Log which users/devices we're sending keys to.
            let recipient_count: usize = request.messages.values()
                .map(|devices| devices.len())
                .sum();
            let recipients: Vec<String> = request.messages.keys()
                .map(|u| u.to_string())
                .collect();
            info!(
                room_id = %room_id,
                event_type = %request.event_type,
                recipients = recipient_count,
                recipient_users = ?recipients,
                "encrypt: sending to-device key share"
            );
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

        // Process any remaining outgoing requests (e.g., key uploads).
        self.process_outgoing_requests().await?;

        let encrypted_value: Value = serde_json::from_str(encrypted.json().get())?;

        // Log key fields from encrypted content for debugging.
        info!(
            room_id = %room_id,
            session_id = encrypted_value.get("session_id").and_then(|v| v.as_str()).unwrap_or("?"),
            has_sender_key = encrypted_value.get("sender_key").is_some(),
            has_device_id = encrypted_value.get("device_id").is_some(),
            algorithm = encrypted_value.get("algorithm").and_then(|v| v.as_str()).unwrap_or("?"),
            "encrypt: message encrypted successfully"
        );

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
        match matrix_client.get_room_encryption_event(room_id.as_str()).await {
            Ok(Some(_)) => {
                // Server says encrypted but we didn't know locally — sync our state.
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

    /// Track users so their device keys are queried and kept up-to-date.
    ///
    /// Also claims any missing Olm sessions for the tracked users.
    /// Call this when the bridge bot joins a room — all room members should be
    /// tracked so we can receive Megolm session keys from their devices.
    pub async fn update_tracked_users(&self, user_ids: &[OwnedUserId]) -> anyhow::Result<()> {
        let _guard = self.lock.write().await;

        let refs: Vec<&UserId> = user_ids.iter().map(|u| u.as_ref()).collect();
        self.machine.update_tracked_users(refs).await?;

        // Process the resulting key query requests.
        self.process_outgoing_requests().await?;

        // Claim any missing sessions so we're ready to decrypt.
        self.claim_missing_sessions(user_ids).await?;

        debug!(count = user_ids.len(), "tracked users updated");
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
