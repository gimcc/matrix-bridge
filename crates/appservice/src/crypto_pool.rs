use std::collections::BTreeMap;
use std::sync::Arc;

use ruma::{
    OwnedUserId, OneTimeKeyAlgorithm, UInt, UserId,
    api::client::sync::sync_events::DeviceLists,
    events::AnyToDeviceEvent, serde::Raw,
};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

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
    /// Passphrase for crypto stores.
    passphrase: Option<String>,
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
            passphrase: passphrase.map(|s| s.to_string()),
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
        // Bot user always uses the bot's CryptoManager.
        if user_id == self.bot.user_id() {
            return Ok(Arc::clone(&self.bot));
        }

        // Single-device mode: all puppets use the bot's CryptoManager.
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

        // Register the puppet with its own device on the homeserver.
        let localpart = user_id.localpart();
        self.matrix_client
            .register_puppet_with_device(localpart, Some(device_id.as_str()))
            .await?;

        // Create a per-user MatrixClient clone.
        let puppet_client = self.matrix_client.with_user_device(
            user_id.as_str(),
            device_id.as_str(),
        );

        // Create the CryptoManager with per-puppet store.
        let cm = CryptoManager::new_for_puppet(
            user_id,
            &device_id,
            &self.base_store_path,
            self.passphrase.as_deref(),
            puppet_client,
        )
        .await?;

        // Bootstrap cross-signing so the puppet's device is signed by its
        // own self-signing key (removes "not verified by its owner" warning).
        if let Err(e) = cm.bootstrap_cross_signing(false).await {
            warn!(
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

    /// Process MSC3202 per-user sync changes from a transaction.
    ///
    /// Routes to-device events by recipient user, applies per-user OTK counts,
    /// and broadcasts device list changes to all initialized CryptoManagers.
    pub async fn receive_sync_changes(
        &self,
        to_device_events: Vec<Raw<AnyToDeviceEvent>>,
        changed_devices: &DeviceLists,
        otk_counts: &BTreeMap<OneTimeKeyAlgorithm, UInt>,
        fallback_keys: Option<&[OneTimeKeyAlgorithm]>,
        per_user_otk_counts: &std::collections::HashMap<OwnedUserId, BTreeMap<OneTimeKeyAlgorithm, UInt>>,
        per_user_fallback_keys: &std::collections::HashMap<OwnedUserId, Option<Vec<OneTimeKeyAlgorithm>>>,
        per_user_to_device: &std::collections::HashMap<OwnedUserId, Vec<Raw<AnyToDeviceEvent>>>,
    ) -> anyhow::Result<()> {
        if !self.per_user {
            // Single-device mode: all events go to bot.
            return self.bot.receive_sync_changes(
                to_device_events,
                changed_devices,
                otk_counts,
                fallback_keys,
            ).await;
        }

        // Per-user mode:
        // 1. Route to-device events to the correct puppet's OlmMachine.
        // 2. Apply per-user OTK counts.
        // 3. Broadcast device list changes to all initialized CryptoManagers.

        let bot_user_id = self.bot.user_id().to_owned();

        // Collect bot's data. Also route any un-routed to-device events
        // (those without to_user_id) to the bot as a fallback.
        let mut bot_to_device = per_user_to_device
            .get(&bot_user_id)
            .cloned()
            .unwrap_or_default();
        // Append un-routed events (from legacy homeserver without to_user_id).
        if per_user_to_device.is_empty() && !to_device_events.is_empty() {
            bot_to_device = to_device_events;
        }
        let bot_otk = per_user_otk_counts
            .get(&bot_user_id)
            .cloned()
            .unwrap_or_else(|| otk_counts.clone());
        let bot_fallback = per_user_fallback_keys
            .get(&bot_user_id)
            .cloned()
            .flatten()
            .or_else(|| fallback_keys.map(|s| s.to_vec()));

        // Bot always gets device list changes + its own to-device + OTK.
        if let Err(e) = self.bot.receive_sync_changes(
            bot_to_device,
            changed_devices,
            &bot_otk,
            bot_fallback.as_deref(),
        ).await {
            error!("bot crypto sync failed: {e}");
        }

        // Collect all puppet user IDs that need processing (from to-device, OTK, or fallback).
        let puppet_user_ids: Vec<OwnedUserId> = per_user_to_device
            .keys()
            .chain(per_user_otk_counts.keys())
            .chain(per_user_fallback_keys.keys())
            .filter(|uid| **uid != bot_user_id)
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        for user_id in &puppet_user_ids {
            let puppet_to_device = per_user_to_device
                .get(user_id)
                .cloned()
                .unwrap_or_default();
            let has_to_device = !puppet_to_device.is_empty();

            // Eagerly initialize puppets that receive to-device events.
            // To-device events (Olm pre-key messages) are non-retryable — if we
            // don't process them now, the Olm session key is permanently lost.
            let cm = if has_to_device {
                match self.get_or_init(user_id).await {
                    Ok(cm) => cm,
                    Err(e) => {
                        error!(
                            user_id = %user_id,
                            event_count = puppet_to_device.len(),
                            "failed to auto-init puppet for to-device events: {e}"
                        );
                        continue;
                    }
                }
            } else {
                // For OTK/fallback-only updates, only process if already initialized.
                match self.get(user_id).await {
                    Some(cm) => cm,
                    None => continue,
                }
            };

            let puppet_otk = per_user_otk_counts
                .get(user_id)
                .cloned()
                .unwrap_or_default();
            let puppet_fallback = per_user_fallback_keys
                .get(user_id)
                .cloned()
                .flatten();

            if let Err(e) = cm.receive_sync_changes(
                puppet_to_device,
                changed_devices,
                &puppet_otk,
                puppet_fallback.as_deref(),
            ).await {
                warn!(user_id = %user_id, "puppet crypto sync failed: {e}");
            }
        }

        // Broadcast device list changes to initialized puppets that didn't
        // already receive sync changes above.
        if !changed_devices.changed.is_empty() || !changed_devices.left.is_empty() {
            let puppets = self.puppets.read().await;
            let empty_otk = BTreeMap::new();
            for (user_id, cm) in puppets.iter() {
                if *user_id == bot_user_id || puppet_user_ids.contains(user_id) {
                    continue;
                }
                if let Err(e) = cm.receive_sync_changes(
                    vec![],
                    changed_devices,
                    &empty_otk,
                    None,
                ).await {
                    warn!(user_id = %user_id, "puppet device list sync failed: {e}");
                }
            }
        }

        Ok(())
    }

    /// Encrypt a message using the appropriate CryptoManager.
    ///
    /// In per-user mode, uses the puppet's own OlmMachine.
    /// In single-device mode, uses the bot's OlmMachine.
    pub async fn encrypt(
        &self,
        sender_user_id: &UserId,
        room_id: &ruma::RoomId,
        event_type: &str,
        content: &Value,
        room_members: &[OwnedUserId],
    ) -> anyhow::Result<Value> {
        let cm = self.get_or_init(sender_user_id).await?;
        cm.encrypt(room_id, event_type, content, room_members).await
    }

    /// Decrypt a message. Tries the bot's CryptoManager first.
    pub async fn decrypt(
        &self,
        room_id: &ruma::RoomId,
        event: &Value,
    ) -> anyhow::Result<crate::crypto_manager::DecryptedEvent> {
        // Bot is always in the room and should have the Megolm session key.
        self.bot.decrypt(room_id, event).await
    }

    /// Process outgoing requests for all initialized CryptoManagers.
    pub async fn process_all_outgoing_requests(&self) -> anyhow::Result<()> {
        self.bot.process_outgoing_requests().await?;

        let puppets = self.puppets.read().await;
        for (user_id, cm) in puppets.iter() {
            if let Err(e) = cm.process_outgoing_requests().await {
                warn!(user_id = %user_id, "puppet outgoing requests failed: {e}");
            }
        }
        Ok(())
    }

    /// Get the device_id for a puppet user (for send_encrypted_message).
    ///
    /// Looks up the initialized CryptoManager to get the actual device_id,
    /// ensuring consistency with the device keys that were uploaded.
    pub async fn device_id_for_user(&self, user_id: &UserId) -> Option<String> {
        if !self.per_user || user_id == self.bot.user_id() {
            return Some(self.bot.device_id().as_str().to_string());
        }
        let puppets = self.puppets.read().await;
        puppets
            .get(user_id)
            .map(|cm| cm.device_id().as_str().to_string())
    }
}
