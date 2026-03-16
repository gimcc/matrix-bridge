mod bootstrap;
mod decrypt;
mod encrypt;
mod keys;
mod sync;

use std::path::Path;

use matrix_sdk_crypto::OlmMachine;
use matrix_sdk_sqlite::SqliteCryptoStore;
use ruma::{OwnedDeviceId, UserId};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use matrix_bridge_core::config::EncryptionConfig;

use crate::matrix_client::MatrixClient;

pub use self::decrypt::DecryptedEvent;

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

        let mut cm = Self::open(
            user_id,
            device_id,
            store_path,
            passphrase,
            matrix_client.clone(),
        )
        .await?;

        // Upload any pending keys (fresh store) or process OTK replenishment.
        cm.process_outgoing_requests().await?;

        // Verify that our device keys are actually on the homeserver.
        if !cm.device_keys_on_server().await? {
            warn!("device keys missing on server — rebuilding crypto store");
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
        if let Err(e) = cm.bootstrap_cross_signing(false).await {
            warn!("cross-signing bootstrap failed (non-fatal): {e}");
        }

        Ok(cm)
    }

    /// Initialize a crypto manager for a puppet user (per-user crypto mode).
    ///
    /// Unlike `new()`, this uses a per-puppet subdirectory for the crypto store
    /// and does not attempt the "device keys on server" rebuild loop.
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

        let localpart = user_id.localpart();
        let dir_name = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(localpart.as_bytes());
            hash[..16]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let store_path = Path::new(base_store_path).join("puppets").join(dir_name);
        std::fs::create_dir_all(&store_path)?;

        let cm = Self::open(user_id, device_id, &store_path, passphrase, matrix_client).await?;

        cm.process_outgoing_requests().await?;

        info!(
            user_id = %user_id,
            device_id = %device_id,
            "puppet crypto manager initialized"
        );

        Ok(cm)
    }

    /// Generate a deterministic device ID for a puppet user.
    pub fn puppet_device_id(user_id: &UserId, prefix: &str) -> OwnedDeviceId {
        use sha2::{Digest, Sha256};
        let localpart = user_id.localpart();
        let hash = Sha256::digest(localpart.as_bytes());
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

    /// Get the bridge bot's device ID.
    pub fn device_id(&self) -> &ruma::DeviceId {
        self.machine.device_id()
    }

    /// Get the bridge bot's user ID.
    pub fn user_id(&self) -> &UserId {
        self.machine.user_id()
    }
}
