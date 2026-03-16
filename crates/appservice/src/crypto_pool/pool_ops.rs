use std::collections::{BTreeMap, HashMap};

use ruma::{
    OneTimeKeyAlgorithm, OwnedUserId, UInt, UserId, api::client::sync::sync_events::DeviceLists,
    events::AnyToDeviceEvent, serde::Raw,
};
use serde_json::Value;
use tracing::{error, warn};

use super::CryptoManagerPool;

/// Aggregated MSC2409/MSC3202 sync data from a single transaction.
pub struct SyncChanges {
    pub to_device_events: Vec<Raw<AnyToDeviceEvent>>,
    pub changed_devices: DeviceLists,
    pub otk_counts: BTreeMap<OneTimeKeyAlgorithm, UInt>,
    pub fallback_keys: Option<Vec<OneTimeKeyAlgorithm>>,
    pub per_user_otk_counts: HashMap<OwnedUserId, BTreeMap<OneTimeKeyAlgorithm, UInt>>,
    pub per_user_fallback_keys: HashMap<OwnedUserId, Option<Vec<OneTimeKeyAlgorithm>>>,
    pub per_user_to_device: HashMap<OwnedUserId, Vec<Raw<AnyToDeviceEvent>>>,
}

impl CryptoManagerPool {
    /// Process MSC3202 per-user sync changes from a transaction.
    ///
    /// Routes to-device events by recipient user, applies per-user OTK counts,
    /// and broadcasts device list changes to all initialized CryptoManagers.
    pub async fn receive_sync_changes(&self, changes: SyncChanges) -> anyhow::Result<()> {
        let SyncChanges {
            to_device_events,
            changed_devices,
            otk_counts,
            fallback_keys,
            per_user_otk_counts,
            per_user_fallback_keys,
            per_user_to_device,
        } = &changes;
        if !self.per_user {
            return self
                .bot
                .receive_sync_changes(
                    to_device_events.clone(),
                    changed_devices,
                    otk_counts,
                    fallback_keys.as_deref(),
                )
                .await;
        }

        let bot_user_id = self.bot.user_id().to_owned();

        let mut bot_to_device = per_user_to_device
            .get(&bot_user_id)
            .cloned()
            .unwrap_or_default();
        if per_user_to_device.is_empty() && !to_device_events.is_empty() {
            bot_to_device = to_device_events.clone();
        }
        let bot_otk = per_user_otk_counts
            .get(&bot_user_id)
            .cloned()
            .unwrap_or_else(|| otk_counts.clone());
        let bot_fallback = per_user_fallback_keys
            .get(&bot_user_id)
            .cloned()
            .flatten()
            .or_else(|| fallback_keys.clone());

        if let Err(e) = self
            .bot
            .receive_sync_changes(
                bot_to_device,
                changed_devices,
                &bot_otk,
                bot_fallback.as_deref(),
            )
            .await
        {
            error!(error = %e, "bot crypto sync failed");
        }

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
            let puppet_to_device = per_user_to_device.get(user_id).cloned().unwrap_or_default();
            let has_to_device = !puppet_to_device.is_empty();

            let cm = if has_to_device {
                match self.get_or_init(user_id).await {
                    Ok(cm) => cm,
                    Err(e) => {
                        error!(
                            user_id = %user_id,
                            event_count = puppet_to_device.len(),
                            error = %e,
                            "failed to auto-init puppet for to-device events"
                        );
                        continue;
                    }
                }
            } else {
                match self.get(user_id).await {
                    Some(cm) => cm,
                    None => continue,
                }
            };

            let puppet_otk = per_user_otk_counts
                .get(user_id)
                .cloned()
                .unwrap_or_default();
            let puppet_fallback = per_user_fallback_keys.get(user_id).cloned().flatten();

            if let Err(e) = cm
                .receive_sync_changes(
                    puppet_to_device,
                    changed_devices,
                    &puppet_otk,
                    puppet_fallback.as_deref(),
                )
                .await
            {
                warn!(user_id = %user_id, error = %e, "puppet crypto sync failed");
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
                if let Err(e) = cm
                    .receive_sync_changes(vec![], changed_devices, &empty_otk, None)
                    .await
                {
                    warn!(user_id = %user_id, error = %e, "puppet device list sync failed");
                }
            }
        }

        Ok(())
    }

    /// Encrypt a message using the appropriate CryptoManager.
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
        self.bot.decrypt(room_id, event).await
    }

    /// Process outgoing requests for all initialized CryptoManagers.
    pub async fn process_all_outgoing_requests(&self) -> anyhow::Result<()> {
        self.bot.process_outgoing_requests().await?;

        let puppets = self.puppets.read().await;
        for (user_id, cm) in puppets.iter() {
            if let Err(e) = cm.process_outgoing_requests().await {
                warn!(user_id = %user_id, error = %e, "puppet outgoing requests failed");
            }
        }
        Ok(())
    }

    /// Get the device_id for a puppet user (for send_encrypted_message).
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
