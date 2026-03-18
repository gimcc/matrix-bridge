use std::collections::BTreeMap;
use std::path::Path;

use matrix_sdk_crypto::{
    CrossSigningBootstrapRequests, DecryptionSettings, EncryptionSettings, EncryptionSyncChanges,
    OlmMachine, TrustRequirement, store::types::RoomSettings,
    types::EventEncryptionAlgorithm,
    types::requests::{AnyIncomingResponse, AnyOutgoingRequest},
};
use matrix_sdk_sqlite::SqliteCryptoStore;
use ruma::{
    OneTimeKeyAlgorithm, OwnedDeviceId, OwnedUserId, RoomId, UInt, UserId,
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

        // Bootstrap cross-signing if not already done.
        // This creates MSK/SSK/USK, uploads them, and signs our device key
        // with the self-signing key so clients see the device as verified.
        if let Err(e) = cm.bootstrap_cross_signing(false).await {
            warn!("cross-signing bootstrap failed (non-fatal): {e}");
        }

        Ok(cm)
    }

    /// Initialize a crypto manager for a puppet user (per-user crypto mode).
    ///
    /// Unlike `new()`, this uses a per-puppet subdirectory for the crypto store
    /// and does not attempt the "device keys on server" rebuild loop.
    /// The `matrix_client` should be a per-user clone from `with_user_device()`.
    pub async fn new_for_puppet(
        user_id: &UserId,
        device_id: &ruma::DeviceId,
        base_store_path: &str,
        passphrase: Option<&str>,
        matrix_client: MatrixClient,
    ) -> anyhow::Result<Self> {
        if passphrase.is_none() || passphrase == Some("") {
            anyhow::bail!(
                "encryption.crypto_store_passphrase must be set for puppet crypto stores. \
                 Without a passphrase, Olm/Megolm keys are stored unencrypted on disk."
            );
        }

        // Per-puppet store under base_store_path/puppets/{hash}/
        // Uses SHA-256 of the localpart as directory name to prevent path traversal
        // (localparts can contain '/' and '..' per the Matrix spec).
        let localpart = user_id.localpart();
        let dir_name = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(localpart.as_bytes());
            hash[..16].iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let store_path = Path::new(base_store_path).join("puppets").join(dir_name);
        std::fs::create_dir_all(&store_path)?;

        let cm = Self::open(user_id, device_id, &store_path, passphrase, matrix_client).await?;

        // Upload device keys and OTKs.
        cm.process_outgoing_requests().await?;

        info!(
            user_id = %user_id,
            device_id = %device_id,
            "puppet crypto manager initialized"
        );

        Ok(cm)
    }

    /// Generate a deterministic device ID for a puppet user.
    ///
    /// Uses `{prefix}_{sha256(localpart)[0..8] as hex}` to stay under the 64-char
    /// limit and remain stable across Rust toolchain upgrades (SHA-256 output is
    /// guaranteed stable, unlike `DefaultHasher`).
    pub fn puppet_device_id(user_id: &UserId, prefix: &str) -> OwnedDeviceId {
        use sha2::{Digest, Sha256};
        let localpart = user_id.localpart();
        let hash = Sha256::digest(localpart.as_bytes());
        // 8 bytes = 16 hex chars, 64 bits of collision resistance.
        let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
        let device_id_str = format!("{prefix}_{hex}");
        device_id_str.into()
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
            AnyOutgoingRequest::SignatureUpload(req) => {
                let signed_keys_json = serde_json::to_value(&req.signed_keys)?;
                self.matrix_client.upload_signatures(&signed_keys_json).await?;
                let resp = ruma::api::client::keys::upload_signatures::v3::Response::new();
                self.machine
                    .mark_request_as_sent(request_id, &resp)
                    .await?;
                info!("cross-signing signatures uploaded via outgoing request");
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

        debug!(
            room_id = %room_id,
            member_count = room_members.len(),
            "encrypt: starting preparation"
        );

        // Step 1: Ensure all room members are tracked.
        // OlmMachine internally marks new users as needing a key query; for
        // already-tracked users it is a no-op.  Device key freshness is
        // maintained via device_lists.changed in receive_sync_changes().
        let refs: Vec<&UserId> = room_members.iter().map(|u| u.as_ref()).collect();
        self.machine.update_tracked_users(refs).await?;

        // Step 2: Process pending outgoing requests (key uploads, key queries
        // for newly-tracked users, etc.) in a single batch.
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

        // NOTE: We skip process_outgoing_requests() here.
        // The transaction-level flush in server.rs handles any remaining
        // outgoing requests (key uploads, etc.) after all events are processed.

        let encrypted_value: Value = serde_json::from_str(encrypted.json().get())?;

        debug!(
            room_id = %room_id,
            session_id = encrypted_value.get("session_id").and_then(|v| v.as_str()).unwrap_or("?"),
            "encrypt: done"
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

    /// Bootstrap cross-signing keys (master, self-signing, user-signing).
    ///
    /// Generates the cross-signing identity, uploads the signing keys to the
    /// homeserver, and uploads device key signatures so that the device is
    /// verified by its owner's self-signing key.
    ///
    /// If cross-signing is already set up (`reset = false`), re-uploads the
    /// existing identity and re-signs the current device.
    pub async fn bootstrap_cross_signing(&self, reset: bool) -> anyhow::Result<()> {
        let status = self.machine.cross_signing_status().await;
        let has_master = status.has_master;
        let has_self_signing = status.has_self_signing;
        let has_user_signing = status.has_user_signing;

        info!(
            has_master,
            has_self_signing,
            has_user_signing,
            reset,
            "cross-signing status before bootstrap"
        );

        let CrossSigningBootstrapRequests {
            upload_keys_req,
            upload_signing_keys_req,
            upload_signatures_req,
        } = self.machine.bootstrap_cross_signing(reset).await?;

        // Step 1: Upload device keys if needed (fresh account).
        if let Some(req) = upload_keys_req {
            self.dispatch_outgoing_request(req.request_id(), req.request()).await?;
            info!("cross-signing: device keys uploaded");
        }

        // Step 2: Upload cross-signing public keys to
        // POST /_matrix/client/v3/keys/device_signing/upload
        {
            let master_json = upload_signing_keys_req
                .master_key
                .as_ref()
                .map(|k| serde_json::to_value(k))
                .transpose()?;
            let self_signing_json = upload_signing_keys_req
                .self_signing_key
                .as_ref()
                .map(|k| serde_json::to_value(k))
                .transpose()?;
            let user_signing_json = upload_signing_keys_req
                .user_signing_key
                .as_ref()
                .map(|k| serde_json::to_value(k))
                .transpose()?;

            self.matrix_client
                .upload_signing_keys(
                    master_json.as_ref(),
                    self_signing_json.as_ref(),
                    user_signing_json.as_ref(),
                )
                .await?;

            // Notify OlmMachine that signing keys were uploaded.
            // SigningKeysUploadResponse has no From impl for AnyIncomingResponse,
            // so we construct it directly.
            let resp = ruma::api::client::keys::upload_signing_keys::v3::Response::new();
            self.machine
                .mark_request_as_sent(
                    &ruma::TransactionId::new(),
                    AnyIncomingResponse::SigningKeysUpload(&resp),
                )
                .await?;

            info!("cross-signing: signing keys uploaded");
        }

        // Step 3: Upload signatures (self-signing key signs device keys).
        // POST /_matrix/client/v3/keys/signatures/upload
        {
            let signed_keys_json = serde_json::to_value(&upload_signatures_req.signed_keys)?;
            self.matrix_client
                .upload_signatures(&signed_keys_json)
                .await?;

            let resp = ruma::api::client::keys::upload_signatures::v3::Response::new();
            self.machine
                .mark_request_as_sent(&ruma::TransactionId::new(), &resp)
                .await?;

            info!("cross-signing: device signatures uploaded");
        }

        info!(
            user_id = %self.machine.user_id(),
            device_id = %self.machine.device_id(),
            "cross-signing bootstrap complete"
        );

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

    /// Query encryption status: cross-signing state and device keys from server.
    ///
    /// Returns `(cross_signing_status, server_device_info)` where device info
    /// includes algorithms, identity keys (curve25519/ed25519), and signatures.
    pub async fn crypto_status(&self) -> anyhow::Result<CryptoStatus> {
        let cross_signing = self.machine.cross_signing_status().await;

        // Query homeserver for our device keys (includes OTK counts in practice,
        // but the keys/query response gives us the uploaded device key details).
        let user_id = self.machine.user_id().to_owned();
        let device_id = self.machine.device_id().to_owned();
        let req = matrix_sdk_crypto::types::requests::KeysQueryRequest {
            device_keys: BTreeMap::from([(user_id.clone(), Vec::new())]),
            timeout: Some(std::time::Duration::from_secs(10)),
        };
        let resp = self.matrix_client.query_keys_raw(&req).await?;

        let device_info = resp
            .device_keys
            .get(&user_id)
            .and_then(|devices| devices.get(&device_id))
            .map(|raw| serde_json::to_value(raw).unwrap_or_default());

        Ok(CryptoStatus {
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            has_master_key: cross_signing.has_master,
            has_self_signing_key: cross_signing.has_self_signing,
            has_user_signing_key: cross_signing.has_user_signing,
            device_keys_uploaded: device_info.is_some(),
            device_keys: device_info,
        })
    }
}

/// Encryption status for a single crypto device (bot or puppet).
#[derive(Debug, serde::Serialize)]
pub struct CryptoStatus {
    pub user_id: String,
    pub device_id: String,
    pub has_master_key: bool,
    pub has_self_signing_key: bool,
    pub has_user_signing_key: bool,
    pub device_keys_uploaded: bool,
    /// Raw device keys from the homeserver (algorithms, keys, signatures).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_keys: Option<Value>,
}

/// Result of decrypting an event.
pub struct DecryptedEvent {
    pub event_type: String,
    pub content: Value,
    pub sender: String,
}
