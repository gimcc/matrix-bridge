mod pool_ops;

pub use pool_ops::SyncChanges;

use std::sync::Arc;

use ruma::{OwnedUserId, UserId};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::RwLock;
use tracing::info;

use crate::crypto_manager::CryptoManager;
use crate::matrix_client::MatrixClient;

/// Manages per-user OlmMachine instances for per-user crypto mode.
///
/// The bridge bot always has its own CryptoManager (initialized at startup).
/// Puppet users get lazily-initialized CryptoManagers when they first enter
/// an encrypted room.
///
/// When `per_user_crypto` is disabled, all operations delegate to the bridge
/// bot's CryptoManager (backward-compatible single-device mode).
pub struct CryptoManagerPool {
    /// Bridge bot's own CryptoManager (always initialized at startup).
    bot: Arc<CryptoManager>,
    /// Per-puppet CryptoManagers, lazily initialized.
    puppets: RwLock<std::collections::HashMap<OwnedUserId, Arc<CryptoManager>>>,
    /// Base MatrixClient (cloned per-puppet via with_user_device).
    matrix_client: MatrixClient,
    /// Base path for crypto stores.
    base_store_path: String,
    /// Passphrase for crypto stores (wrapped for zeroize-on-drop).
    passphrase: Option<SecretString>,
    /// Prefix for generating puppet device IDs.
    device_prefix: String,
    /// Whether per-user crypto is enabled.
    per_user: bool,
    /// Puppet localpart prefix (e.g. "bot") for identifying appservice users.
    /// Reserved for Phase 7 optimization: skip Olm sessions between appservice users.
    #[allow(dead_code)]
    puppet_localpart_prefix: String,
}

impl CryptoManagerPool {
    pub fn new(
        bot: Arc<CryptoManager>,
        matrix_client: MatrixClient,
        base_store_path: &str,
        passphrase: Option<&str>,
        device_prefix: &str,
        per_user: bool,
        puppet_localpart_prefix: &str,
    ) -> Self {
        Self {
            bot,
            puppets: RwLock::new(std::collections::HashMap::new()),
            matrix_client,
            base_store_path: base_store_path.to_string(),
            passphrase: passphrase.map(|s| SecretString::from(s.to_string())),
            device_prefix: device_prefix.to_string(),
            per_user,
            puppet_localpart_prefix: puppet_localpart_prefix.to_string(),
        }
    }

    /// Returns the bridge bot's CryptoManager.
    pub fn bot(&self) -> &Arc<CryptoManager> {
        &self.bot
    }

    /// Whether per-user crypto mode is enabled.
    pub fn is_per_user(&self) -> bool {
        self.per_user
    }

    /// Get an existing CryptoManager for a puppet (without initializing).
    pub async fn get(&self, user_id: &UserId) -> Option<Arc<CryptoManager>> {
        if user_id == self.bot.user_id() {
            return Some(Arc::clone(&self.bot));
        }
        if !self.per_user {
            return Some(Arc::clone(&self.bot));
        }
        let puppets = self.puppets.read().await;
        puppets.get(user_id).map(Arc::clone)
    }

    /// Get or lazily initialize a CryptoManager for a puppet user.
    ///
    /// If `per_user_crypto` is disabled, always returns the bot's CryptoManager.
    /// If `user_id` matches the bot, returns the bot's CryptoManager.
    pub async fn get_or_init(&self, user_id: &UserId) -> anyhow::Result<Arc<CryptoManager>> {
        if user_id == self.bot.user_id() {
            return Ok(Arc::clone(&self.bot));
        }

        if !self.per_user {
            return Ok(Arc::clone(&self.bot));
        }

        // Fast path: check if already initialized.
        {
            let puppets = self.puppets.read().await;
            if let Some(cm) = puppets.get(user_id) {
                return Ok(Arc::clone(cm));
            }
        }

        // Slow path: initialize under write lock (double-check).
        let mut puppets = self.puppets.write().await;
        if let Some(cm) = puppets.get(user_id) {
            return Ok(Arc::clone(cm));
        }

        let owned_user_id: OwnedUserId = user_id.to_owned();
        let device_id = CryptoManager::puppet_device_id(user_id, &self.device_prefix);

        info!(
            user_id = %user_id,
            device_id = %device_id,
            "initializing per-user crypto for puppet"
        );

        let localpart = user_id.localpart();
        self.matrix_client
            .register_puppet_with_device(localpart, Some(device_id.as_str()))
            .await?;

        let puppet_client = self
            .matrix_client
            .with_user_device(user_id.as_str(), device_id.as_str());

        let cm = CryptoManager::new_for_puppet(
            user_id,
            &device_id,
            &self.base_store_path,
            self.passphrase.as_ref().map(|s| s.expose_secret().as_ref()),
            puppet_client,
        )
        .await?;

        if let Err(e) = cm.bootstrap_cross_signing(false).await {
            tracing::warn!(
                user_id = %user_id,
                "puppet cross-signing bootstrap failed (non-fatal): {e}"
            );
        }

        let cm = Arc::new(cm);
        puppets.insert(owned_user_id, Arc::clone(&cm));

        Ok(cm)
    }

    /// Get all initialized CryptoManagers (bot + all puppets).
    pub async fn get_all(&self) -> Vec<Arc<CryptoManager>> {
        let mut all = vec![Arc::clone(&self.bot)];
        let puppets = self.puppets.read().await;
        all.extend(puppets.values().map(Arc::clone));
        all
    }
}
